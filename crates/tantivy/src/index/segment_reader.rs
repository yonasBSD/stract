use std::collections::HashMap;
use std::ops::BitOrAssign;
use std::sync::{Arc, RwLock};
use std::{fmt, io};

use fnv::FnvHashMap;
use itertools::Itertools;

use crate::columnfield::ColumnFieldReaders;
use crate::directory::{CompositeFile, FileSlice};
use crate::error::DataCorruption;
use crate::fieldnorm::{FieldNormReader, FieldNormReaders};
use crate::index::{InvertedIndexReader, Segment, SegmentComponent, SegmentId};
use crate::json_utils::json_path_sep_to_dot;
use crate::roworder::readers::RowFieldReaders;
use crate::schema::{Field, IndexRecordOption, Schema, Type};
use crate::space_usage::SegmentSpaceUsage;
use crate::store::StoreReader;
use crate::termdict::TermDictionary;
use crate::DocId;

/// Entry point to access all of the datastructures of the `Segment`
///
/// - term dictionary
/// - postings
/// - store
/// - columnar field readers
/// - field norm reader
///
/// The segment reader has a very low memory footprint,
/// as close to all of the memory data is mmapped.
#[derive(Clone)]
pub struct SegmentReader {
    inv_idx_reader_cache: Arc<RwLock<HashMap<Field, Arc<InvertedIndexReader>>>>,

    segment_id: SegmentId,

    max_doc: DocId,

    termdict_composite: CompositeFile,
    postings_composite: CompositeFile,
    positions_composite: CompositeFile,
    column_fields_readers: ColumnFieldReaders,
    row_fields_readers: RowFieldReaders,
    fieldnorm_readers: FieldNormReaders,

    store_file: FileSlice,
    schema: Schema,
}

impl SegmentReader {
    /// Returns the highest document id ever attributed in
    /// this segment + 1.
    pub fn max_doc(&self) -> DocId {
        self.max_doc
    }

    /// Returns the number of documents.
    pub fn num_docs(&self) -> DocId {
        self.max_doc()
    }

    /// Returns the schema of the index this segment belongs to.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Accessor to a segment's columnar field reader given a field.
    ///
    /// Returns the u64 columnar value reader if the field
    /// is a u64 field indexed as "columnar".
    ///
    /// Return a ColumnFieldNotAvailableError if the field is not
    /// declared as a columnar field in the schema.
    ///
    /// # Panics
    /// May panic if the index is corrupted.
    pub fn column_fields(&self) -> &ColumnFieldReaders {
        &self.column_fields_readers
    }

    /// Accessor to the segment's row field reader.
    pub fn row_fields(&self) -> &RowFieldReaders {
        &self.row_fields_readers
    }

    /// Accessor to the segment's `Field norms`'s reader.
    ///
    /// Field norms are the length (in tokens) of the fields.
    /// It is used in the computation of the [TfIdf](https://fulmicoton.gitbooks.io/tantivy-doc/content/tfidf.html).
    ///
    /// They are simply stored as a columnar field, serialized in
    /// the `.fieldnorm` file of the segment.
    pub fn get_fieldnorms_reader(&self, field: Field) -> crate::Result<FieldNormReader> {
        self.fieldnorm_readers.get_field(field)?.ok_or_else(|| {
            let field_name = self.schema.get_field_name(field);
            let err_msg = format!(
                "Field norm not found for field {field_name:?}. Was the field set to record norm \
                 during indexing?"
            );
            crate::TantivyError::SchemaError(err_msg)
        })
    }

    #[doc(hidden)]
    pub fn fieldnorms_readers(&self) -> &FieldNormReaders {
        &self.fieldnorm_readers
    }

    /// Accessor to the segment's [`StoreReader`](crate::store::StoreReader).
    ///
    /// `cache_num_blocks` sets the number of decompressed blocks to be cached in an LRU.
    /// The size of blocks is configurable, this should be reflexted in the
    pub fn get_store_reader(&self, cache_num_blocks: usize) -> io::Result<StoreReader> {
        StoreReader::open(self.store_file.clone(), cache_num_blocks)
    }

    /// Open a new segment for reading.
    pub fn open(segment: &Segment) -> crate::Result<SegmentReader> {
        let termdict_file = segment.open_read(SegmentComponent::Terms)?;
        let termdict_composite = CompositeFile::open(&termdict_file)?;

        let store_file = segment.open_read(SegmentComponent::Store)?;

        crate::fail_point!("SegmentReader::open#middle");

        let postings_file = segment.open_read(SegmentComponent::Postings)?;
        let postings_composite = CompositeFile::open(&postings_file)?;

        let positions_composite = {
            if let Ok(positions_file) = segment.open_read(SegmentComponent::Positions) {
                CompositeFile::open(&positions_file)?
            } else {
                CompositeFile::empty()
            }
        };

        let schema = segment.schema();

        let column_fields_data = segment.open_read(SegmentComponent::ColumnFields)?;
        let column_fields_readers = ColumnFieldReaders::open(column_fields_data, schema.clone())?;
        let fieldnorm_data = segment.open_read(SegmentComponent::FieldNorms)?;
        let fieldnorm_readers = FieldNormReaders::open(fieldnorm_data)?;
        let row_fields_data = segment.open_read(SegmentComponent::RowFields)?;
        let row_fields_readers = RowFieldReaders::open(row_fields_data)?;

        let max_doc = segment.meta().max_doc();

        Ok(SegmentReader {
            inv_idx_reader_cache: Default::default(),
            max_doc,
            termdict_composite,
            postings_composite,
            column_fields_readers,
            row_fields_readers,
            fieldnorm_readers,
            segment_id: segment.id(),
            store_file,
            positions_composite,
            schema,
        })
    }

    /// Returns a field reader associated with the field given in argument.
    /// If the field was not present in the index during indexing time,
    /// the InvertedIndexReader is empty.
    ///
    /// The field reader is in charge of iterating through the
    /// term dictionary associated with a specific field,
    /// and opening the posting list associated with any term.
    ///
    /// If the field is not marked as index, a warning is logged and an empty `InvertedIndexReader`
    /// is returned.
    /// Similarly, if the field is marked as indexed but no term has been indexed for the given
    /// index, an empty `InvertedIndexReader` is returned (but no warning is logged).
    pub fn inverted_index(&self, field: Field) -> crate::Result<Arc<InvertedIndexReader>> {
        if let Some(inv_idx_reader) = self
            .inv_idx_reader_cache
            .read()
            .expect("Lock poisoned. This should never happen")
            .get(&field)
        {
            return Ok(Arc::clone(inv_idx_reader));
        }
        let field_entry = self.schema.get_field_entry(field);
        let field_type = field_entry.field_type();
        let record_option_opt = field_type.get_index_record_option();

        if record_option_opt.is_none() {
            warn!("Field {:?} does not seem indexed.", field_entry.name());
        }

        let postings_file_opt = self.postings_composite.open_read(field);

        if postings_file_opt.is_none() || record_option_opt.is_none() {
            // no documents in the segment contained this field.
            // As a result, no data is associated with the inverted index.
            //
            // Returns an empty inverted index.
            let record_option = record_option_opt.unwrap_or(IndexRecordOption::Basic);
            return Ok(Arc::new(InvertedIndexReader::empty(record_option)));
        }

        let record_option = record_option_opt.unwrap();
        let postings_file = postings_file_opt.unwrap();

        let termdict_file: FileSlice =
            self.termdict_composite.open_read(field).ok_or_else(|| {
                DataCorruption::comment_only(format!(
                    "Failed to open field {:?}'s term dictionary in the composite file. Has the \
                     schema been modified?",
                    field_entry.name()
                ))
            })?;

        let positions_file = self.positions_composite.open_read(field).ok_or_else(|| {
            let error_msg = format!(
                "Failed to open field {:?}'s positions in the composite file. Has the schema been \
                 modified?",
                field_entry.name()
            );
            DataCorruption::comment_only(error_msg)
        })?;

        let inv_idx_reader = Arc::new(InvertedIndexReader::new(
            TermDictionary::open(termdict_file)?,
            postings_file,
            positions_file,
            record_option,
        )?);

        // by releasing the lock in between, we may end up opening the inverting index
        // twice, but this is fine.
        self.inv_idx_reader_cache
            .write()
            .expect("Field reader cache lock poisoned. This should never happen.")
            .insert(field, Arc::clone(&inv_idx_reader));

        Ok(inv_idx_reader)
    }

    /// Returns the list of fields that have been indexed in the segment.
    /// The field list includes the field defined in the schema as well as the fields
    /// that have been indexed as a part of a JSON field.
    /// The returned field name is the full field name, including the name of the JSON field.
    ///
    /// The returned field names can be used in queries.
    ///
    /// Notice: If your data contains JSON fields this is **very expensive**, as it requires
    /// browsing through the inverted index term dictionary and the columnar field dictionary.
    ///
    /// Disclaimer: Some fields may not be listed here. For instance, if the schema contains a json
    /// field that is not indexed nor a columnar field but is stored, it is possible for the field
    /// to not be listed.
    pub fn fields_metadata(&self) -> crate::Result<Vec<FieldMetadata>> {
        let mut indexed_fields: Vec<FieldMetadata> = Vec::new();
        let mut map_to_canonical = FnvHashMap::default();
        for (field, field_entry) in self.schema().fields() {
            let field_name = field_entry.name().to_string();
            let is_indexed = field_entry.is_indexed();

            if is_indexed {
                let is_json = field_entry.field_type().value_type() == Type::Json;
                if is_json {
                    let inv_index = self.inverted_index(field)?;
                    let encoded_fields_in_index = inv_index.list_encoded_fields()?;
                    let mut build_path = |field_name: &str, mut json_path: String| {
                        // In this case we need to map the potential columnar field to the field name
                        // accepted by the query parser.
                        let create_canonical =
                            !field_entry.is_expand_dots_enabled() && json_path.contains('.');
                        if create_canonical {
                            // Without expand dots enabled dots need to be escaped.
                            let escaped_json_path = json_path.replace('.', "\\.");
                            let full_path = format!("{field_name}.{escaped_json_path}");
                            let full_path_unescaped = format!("{}.{}", field_name, &json_path);
                            map_to_canonical.insert(full_path_unescaped, full_path.to_string());
                            full_path
                        } else {
                            // With expand dots enabled, we can use '.' instead of '\u{1}'.
                            json_path_sep_to_dot(&mut json_path);
                            format!("{field_name}.{json_path}")
                        }
                    };
                    indexed_fields.extend(
                        encoded_fields_in_index
                            .into_iter()
                            .map(|(name, typ)| (build_path(&field_name, name), typ))
                            .map(|(field_name, typ)| FieldMetadata {
                                indexed: true,
                                stored: false,
                                field_name,
                                columnar: false,
                                typ,
                            }),
                    );
                } else {
                    indexed_fields.push(FieldMetadata {
                        indexed: true,
                        stored: false,
                        field_name: field_name.to_string(),
                        columnar: false,
                        typ: field_entry.field_type().value_type(),
                    });
                }
            }
        }
        let mut column_fields: Vec<FieldMetadata> = self
            .column_fields()
            .columnar()
            .iter_columns()?
            .map(|(mut field_name, handle)| {
                json_path_sep_to_dot(&mut field_name);
                // map to canonical path, to avoid similar but different entries.
                // Eventually we should just accept '.' seperated for all cases.
                let field_name = map_to_canonical
                    .get(&field_name)
                    .unwrap_or(&field_name)
                    .to_string();
                FieldMetadata {
                    indexed: false,
                    stored: false,
                    field_name,
                    columnar: true,
                    typ: Type::from(handle.column_type()),
                }
            })
            .collect();
        // Since the type is encoded differently in the columnar field and in the inverted index,
        // the order of the fields is not guaranteed to be the same. Therefore, we sort the fields.
        // If we are sure that the order is the same, we can remove this sort.
        indexed_fields.sort_unstable();
        column_fields.sort_unstable();
        let merged = merge_field_meta_data(vec![indexed_fields, column_fields], &self.schema);

        Ok(merged)
    }

    /// Returns the segment id
    pub fn segment_id(&self) -> SegmentId {
        self.segment_id
    }

    /// Returns an iterator that will iterate over the document ids
    pub fn doc_ids(&self) -> impl Iterator<Item = DocId> + Send + '_ {
        0u32..self.max_doc
    }

    /// Summarize total space usage of this segment.
    pub fn space_usage(&self) -> io::Result<SegmentSpaceUsage> {
        Ok(SegmentSpaceUsage::new(
            self.num_docs(),
            self.termdict_composite.space_usage(),
            self.postings_composite.space_usage(),
            self.positions_composite.space_usage(),
            self.column_fields_readers.space_usage(self.schema())?,
            self.row_fields_readers.space_usage(self.schema())?,
            self.fieldnorm_readers.space_usage(),
            self.get_store_reader(0)?.space_usage(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// FieldMetadata
pub struct FieldMetadata {
    /// The field name
    // Notice: Don't reorder the declaration of 1.field_name 2.typ, as it is used for ordering by
    // field_name then typ.
    pub field_name: String,
    /// The field type
    // Notice: Don't reorder the declaration of 1.field_name 2.typ, as it is used for ordering by
    // field_name then typ.
    pub typ: Type,
    /// Is the field indexed for search
    pub indexed: bool,
    /// Is the field stored in the doc store
    pub stored: bool,
    /// Is the field stored in the columnar storage
    pub columnar: bool,
}
impl BitOrAssign for FieldMetadata {
    fn bitor_assign(&mut self, rhs: Self) {
        assert!(self.field_name == rhs.field_name);
        assert!(self.typ == rhs.typ);
        self.indexed |= rhs.indexed;
        self.stored |= rhs.stored;
        self.columnar |= rhs.columnar;
    }
}

// Maybe too slow for the high cardinality case
fn is_field_stored(field_name: &str, schema: &Schema) -> bool {
    schema
        .find_field(field_name)
        .map(|(field, _path)| schema.get_field_entry(field).is_stored())
        .unwrap_or(false)
}

/// Helper to merge the field metadata from multiple segments.
pub fn merge_field_meta_data(
    field_metadatas: Vec<Vec<FieldMetadata>>,
    schema: &Schema,
) -> Vec<FieldMetadata> {
    let mut merged_field_metadata = Vec::new();
    for (_key, mut group) in &field_metadatas
        .into_iter()
        .kmerge_by(|left, right| left < right)
        // TODO: Remove allocation
        .chunk_by(|el| (el.field_name.to_string(), el.typ))
    {
        let mut merged: FieldMetadata = group.next().unwrap();
        for el in group {
            merged |= el;
        }
        // Currently is_field_stored is maybe too slow for the high cardinality case
        merged.stored = is_field_stored(&merged.field_name, schema);
        merged_field_metadata.push(merged);
    }
    merged_field_metadata
}

impl fmt::Debug for SegmentReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SegmentReader({:?})", self.segment_id)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::index::Index;
    use crate::schema::{SchemaBuilder, STORED, TEXT};
    use crate::IndexWriter;

    #[test]
    fn test_merge_field_meta_data_same() {
        let schema = SchemaBuilder::new().build();
        let field_metadata1 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: true,
            stored: false,
            columnar: true,
        };
        let field_metadata2 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: true,
            stored: false,
            columnar: true,
        };
        let res = merge_field_meta_data(
            vec![vec![field_metadata1.clone()], vec![field_metadata2]],
            &schema,
        );
        assert_eq!(res, vec![field_metadata1]);
    }
    #[test]
    fn test_merge_field_meta_data_different() {
        let schema = SchemaBuilder::new().build();
        let field_metadata1 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: false,
            stored: false,
            columnar: true,
        };
        let field_metadata2 = FieldMetadata {
            field_name: "b".to_string(),
            typ: crate::schema::Type::Str,
            indexed: false,
            stored: false,
            columnar: true,
        };
        let field_metadata3 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: true,
            stored: false,
            columnar: false,
        };
        let res = merge_field_meta_data(
            vec![
                vec![field_metadata1.clone(), field_metadata2.clone()],
                vec![field_metadata3],
            ],
            &schema,
        );
        let field_metadata_expected1 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: true,
            stored: false,
            columnar: true,
        };
        assert_eq!(res, vec![field_metadata_expected1, field_metadata2.clone()]);
    }
    #[test]
    fn test_merge_field_meta_data_merge() {
        use pretty_assertions::assert_eq;
        let get_meta_data = |name: &str, typ: Type| FieldMetadata {
            field_name: name.to_string(),
            typ,
            indexed: false,
            stored: false,
            columnar: true,
        };
        let schema = SchemaBuilder::new().build();
        let mut metas = vec![get_meta_data("d", Type::Str), get_meta_data("e", Type::U64)];
        metas.sort();
        let res = merge_field_meta_data(vec![vec![get_meta_data("e", Type::Str)], metas], &schema);
        assert_eq!(
            res,
            vec![
                get_meta_data("d", Type::Str),
                get_meta_data("e", Type::Str),
                get_meta_data("e", Type::U64),
            ]
        );
    }
    #[test]
    fn test_merge_field_meta_data_bitxor() {
        let field_metadata1 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: false,
            stored: false,
            columnar: true,
        };
        let field_metadata2 = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: true,
            stored: false,
            columnar: false,
        };
        let field_metadata_expected = FieldMetadata {
            field_name: "a".to_string(),
            typ: crate::schema::Type::Str,
            indexed: true,
            stored: false,
            columnar: true,
        };
        let mut res1 = field_metadata1.clone();
        res1 |= field_metadata2.clone();
        let mut res2 = field_metadata2.clone();
        res2 |= field_metadata1;
        assert_eq!(res1, field_metadata_expected);
        assert_eq!(res2, field_metadata_expected);
    }

    #[test]
    fn test_num_alive() -> crate::Result<()> {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());
        let name = schema.get_field("name").unwrap();

        {
            let mut index_writer: IndexWriter = index.writer_for_tests()?;
            index_writer.add_document(doc!(name => "tantivy"))?;
            index_writer.add_document(doc!(name => "horse"))?;
            index_writer.add_document(doc!(name => "jockey"))?;
            index_writer.add_document(doc!(name => "cap"))?;

            index_writer.commit()?;
        }
        let searcher = index.reader()?.searcher();
        assert_eq!(4, searcher.segment_reader(0).num_docs());
        assert_eq!(4, searcher.segment_reader(0).max_doc());
        Ok(())
    }
}
