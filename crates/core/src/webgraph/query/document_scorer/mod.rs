// Stract is an open source web search engine.
// Copyright (C) 2024 Stract ApS
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use tantivy::{columnar::Column, DocId, Score, SegmentReader};

use crate::webgraph::{
    schema::{Field, SortScore},
    warmed_column_fields::WarmedColumnFields,
};

pub trait DocumentScorer: Send + Sync + Sized {
    fn for_segment(
        segment: &SegmentReader,
        column_fields: &WarmedColumnFields,
    ) -> tantivy::Result<Self>;
    fn score(&self, doc: DocId) -> Score;
}

pub struct DefaultDocumentScorer {
    column: Column<f64>,
}

impl DocumentScorer for DefaultDocumentScorer {
    fn for_segment(
        segment: &SegmentReader,
        column_fields: &WarmedColumnFields,
    ) -> tantivy::Result<Self> {
        let column = column_fields
            .segment(&segment.segment_id())
            .f64(SortScore)
            .ok_or(tantivy::TantivyError::FieldNotFound(format!(
                "{} column not found",
                SortScore.name()
            )))?;
        Ok(Self { column })
    }

    fn score(&self, doc: DocId) -> Score {
        if doc == tantivy::TERMINATED {
            return 0.0;
        }

        self.column.first(doc).unwrap_or(0.0) as Score
    }
}
