//! IP Fastfields support efficient scanning for range queries.
//! We use this variant only if the fastfield exists, otherwise the default in `range_query` is
//! used, which uses the term dictionary + postings.

use std::net::Ipv6Addr;
use std::ops::{Bound, RangeInclusive};

use crate::columnar::{Column, MonotonicallyMappableToU128};

use crate::query::range_query::fast_field_range_query::RangeDocSet;
use crate::query::{ConstScorer, EmptyScorer, Explanation, Scorer, Weight};
use crate::{DocId, DocSet, Score, SegmentReader, TantivyError};

/// `IPFastFieldRangeWeight` uses the ip address fast field to execute range queries.
pub struct IPFastFieldRangeWeight {
    field: String,
    lower_bound: Bound<Ipv6Addr>,
    upper_bound: Bound<Ipv6Addr>,
}

impl IPFastFieldRangeWeight {
    /// Creates a new IPFastFieldRangeWeight.
    pub fn new(field: String, lower_bound: Bound<Ipv6Addr>, upper_bound: Bound<Ipv6Addr>) -> Self {
        Self {
            field,
            lower_bound,
            upper_bound,
        }
    }
}

impl Weight for IPFastFieldRangeWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> crate::Result<Box<dyn Scorer>> {
        let Some(ip_addr_column): Option<Column<Ipv6Addr>> =
            reader.fast_fields().column_opt(&self.field)?
        else {
            return Ok(Box::new(EmptyScorer));
        };
        let value_range = bound_to_value_range(
            &self.lower_bound,
            &self.upper_bound,
            ip_addr_column.min_value(),
            ip_addr_column.max_value(),
        );
        let docset = RangeDocSet::new(value_range, ip_addr_column);
        Ok(Box::new(ConstScorer::new(docset, boost)))
    }

    fn explain(&self, reader: &SegmentReader, doc: DocId) -> crate::Result<Explanation> {
        let mut scorer = self.scorer(reader, 1.0)?;
        if scorer.seek(doc) != doc {
            return Err(TantivyError::InvalidArgument(format!(
                "Document #({doc}) does not match"
            )));
        }
        let explanation = Explanation::new("Const", scorer.score());
        Ok(explanation)
    }
}

fn bound_to_value_range(
    lower_bound: &Bound<Ipv6Addr>,
    upper_bound: &Bound<Ipv6Addr>,
    min_value: Ipv6Addr,
    max_value: Ipv6Addr,
) -> RangeInclusive<Ipv6Addr> {
    let start_value = match lower_bound {
        Bound::Included(ip_addr) => *ip_addr,
        Bound::Excluded(ip_addr) => Ipv6Addr::from(ip_addr.to_u128() + 1),
        Bound::Unbounded => min_value,
    };

    let end_value = match upper_bound {
        Bound::Included(ip_addr) => *ip_addr,
        Bound::Excluded(ip_addr) => Ipv6Addr::from(ip_addr.to_u128() - 1),
        Bound::Unbounded => max_value,
    };
    start_value..=end_value
}

#[cfg(test)]
pub mod tests {
    use proptest::prelude::ProptestConfig;
    use proptest::strategy::Strategy;
    use proptest::{prop_oneof, proptest};

    use super::*;
    use crate::collector::Count;
    use crate::query::QueryParser;
    use crate::schema::{Schema, FAST, INDEXED, STORED, STRING};
    use crate::{Index, IndexWriter};

    #[derive(Clone, Debug)]
    pub struct Doc {
        pub id: String,
        pub ip: Ipv6Addr,
    }

    fn operation_strategy() -> impl Strategy<Value = Doc> {
        prop_oneof![
            (0u64..10_000u64).prop_map(doc_from_id_1),
            (1u64..10_000u64).prop_map(doc_from_id_2),
        ]
    }

    pub fn doc_from_id_1(id: u64) -> Doc {
        let id = id * 1000;
        Doc {
            // ip != id
            id: id.to_string(),
            ip: Ipv6Addr::from_u128(id as u128),
        }
    }
    fn doc_from_id_2(id: u64) -> Doc {
        let id = id * 1000;
        Doc {
            // ip != id
            id: (id - 1).to_string(),
            ip: Ipv6Addr::from_u128(id as u128),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]
        #[test]
        fn test_ip_range_for_docs_prop(ops in proptest::collection::vec(operation_strategy(), 1..1000)) {
            assert!(test_ip_range_for_docs(&ops).is_ok());
        }
    }

    #[test]
    fn test_ip_range_regression1() {
        let ops = &[doc_from_id_1(0)];
        assert!(test_ip_range_for_docs(ops).is_ok());
    }

    #[test]
    fn test_ip_range_regression2() {
        let ops = &[
            doc_from_id_1(52),
            doc_from_id_1(63),
            doc_from_id_1(12),
            doc_from_id_2(91),
            doc_from_id_2(33),
        ];
        assert!(test_ip_range_for_docs(ops).is_ok());
    }

    #[test]
    fn test_ip_range_regression3() {
        let ops = &[doc_from_id_1(1), doc_from_id_1(2), doc_from_id_1(3)];
        assert!(test_ip_range_for_docs(ops).is_ok());
    }

    #[test]
    fn test_ip_range_regression3_simple() {
        let mut schema_builder = Schema::builder();
        let ips_field = schema_builder.add_ip_addr_field("ips", FAST | INDEXED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut writer: IndexWriter = index.writer_for_tests().unwrap();
        let ip_addrs: Vec<Ipv6Addr> = [1000, 2000, 3000]
            .into_iter()
            .map(Ipv6Addr::from_u128)
            .collect();
        for &ip_addr in &ip_addrs {
            writer
                .add_document(doc!(ips_field=>ip_addr, ips_field=>ip_addr))
                .unwrap();
        }
        writer.commit().unwrap();
        let searcher = index.reader().unwrap().searcher();
        let range_weight = IPFastFieldRangeWeight {
            field: "ips".to_string(),
            lower_bound: Bound::Included(ip_addrs[1]),
            upper_bound: Bound::Included(ip_addrs[2]),
        };
        let count = range_weight.count(searcher.segment_reader(0)).unwrap();
        assert_eq!(count, 2);
    }

    pub fn create_index_from_docs(docs: &[Doc]) -> Index {
        let mut schema_builder = Schema::builder();
        let ip_field = schema_builder.add_ip_addr_field("ip", STORED | FAST);
        let ips_field = schema_builder.add_ip_addr_field("ips", FAST | INDEXED);
        let text_field = schema_builder.add_text_field("id", STRING | STORED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);

        {
            let mut index_writer = index.writer_with_num_threads(2, 60_000_000).unwrap();
            for doc in docs.iter() {
                index_writer
                    .add_document(doc!(
                        ips_field => doc.ip,
                        ips_field => doc.ip,
                        ip_field => doc.ip,
                        text_field => doc.id.to_string(),
                    ))
                    .unwrap();
            }

            index_writer.commit().unwrap();
        }
        index
    }

    fn test_ip_range_for_docs(docs: &[Doc]) -> crate::Result<()> {
        let index = create_index_from_docs(docs);
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();

        let get_num_hits = |query| searcher.search(&query, &Count).unwrap();
        let query_from_text = |text: &str| {
            QueryParser::for_index(&index, vec![])
                .parse_query(text)
                .unwrap()
        };

        let gen_query_inclusive = |field: &str, ip_range: &RangeInclusive<Ipv6Addr>| {
            format!("{field}:[{} TO {}]", ip_range.start(), ip_range.end())
        };

        let test_sample = |sample_docs: &[Doc]| {
            let mut ips: Vec<Ipv6Addr> = sample_docs.iter().map(|doc| doc.ip).collect();
            ips.sort();
            let ip_range = ips[0]..=ips[1];
            let expected_num_hits = docs
                .iter()
                .filter(|doc| (ips[0]..=ips[1]).contains(&doc.ip))
                .count();

            let query = gen_query_inclusive("ip", &ip_range);
            assert_eq!(get_num_hits(query_from_text(&query)), expected_num_hits);

            let query = gen_query_inclusive("ips", &ip_range);
            assert_eq!(get_num_hits(query_from_text(&query)), expected_num_hits);

            // Intersection search
            let id_filter = sample_docs[0].id.to_string();
            let expected_num_hits = docs
                .iter()
                .filter(|doc| ip_range.contains(&doc.ip) && doc.id == id_filter)
                .count();
            let query = format!(
                "{} AND id:{}",
                gen_query_inclusive("ip", &ip_range),
                &id_filter
            );
            assert_eq!(get_num_hits(query_from_text(&query)), expected_num_hits);

            // Intersection search on multivalue ip field
            let id_filter = sample_docs[0].id.to_string();
            let query = format!(
                "{} AND id:{}",
                gen_query_inclusive("ips", &ip_range),
                &id_filter
            );
            assert_eq!(get_num_hits(query_from_text(&query)), expected_num_hits);
        };

        test_sample(&[docs[0].clone(), docs[0].clone()]);
        if docs.len() > 1 {
            test_sample(&[docs[0].clone(), docs[1].clone()]);
            test_sample(&[docs[1].clone(), docs[1].clone()]);
        }
        if docs.len() > 2 {
            test_sample(&[docs[1].clone(), docs[2].clone()]);
        }

        Ok(())
    }
}
