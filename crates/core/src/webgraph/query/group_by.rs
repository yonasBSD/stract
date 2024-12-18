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

use rustc_hash::{FxHashMap, FxHashSet};
use tantivy::query::{BooleanQuery, Occur};

use crate::{
    hyperloglog::HyperLogLog,
    webgraph::{
        schema::{Field, FieldEnum, FromHostId, ToHostId},
        NodeID,
    },
};

use super::{
    collector::{GroupExactCollector, GroupSketchCollector},
    raw, AndFilter, Filter, FilterEnum, Query,
};

#[derive(Debug, Clone, bincode::Encode, bincode::Decode)]
pub enum LinksDirection {
    From(NodeID),
    To(NodeID),
}

#[derive(Debug, Clone, bincode::Encode, bincode::Decode)]
pub struct HostGroupSketchQuery {
    node: LinksDirection,
    group: FieldEnum,
    value: FieldEnum,
    filters: Vec<FilterEnum>,
    skip_self_links: bool,
    deduplicate: bool,
}

impl HostGroupSketchQuery {
    pub fn new<Group: Field, Value: Field>(
        node: LinksDirection,
        group: Group,
        value: Value,
    ) -> Self {
        Self {
            node,
            group: group.into(),
            value: value.into(),
            filters: Vec::new(),
            skip_self_links: true,
            deduplicate: true,
        }
    }

    pub fn backlinks<Group: Field, Value: Field>(node: NodeID, group: Group, value: Value) -> Self {
        Self::new(LinksDirection::To(node), group, value)
    }

    pub fn forwardlinks<Group: Field, Value: Field>(
        node: NodeID,
        group: Group,
        value: Value,
    ) -> Self {
        Self::new(LinksDirection::From(node), group, value)
    }

    pub fn filter<F: Filter>(mut self, filter: F) -> Self {
        self.filters.push(filter.into());
        self
    }

    fn filter_as_and(&self) -> Option<AndFilter> {
        if self.filters.is_empty() {
            None
        } else {
            let mut filter = AndFilter::new();

            for f in self.filters.clone() {
                filter = filter.and(f);
            }

            Some(filter)
        }
    }

    pub fn skip_self_links(mut self, skip_self_links: bool) -> Self {
        self.skip_self_links = skip_self_links;
        self
    }

    pub fn deduplicate(mut self, deduplicate: bool) -> Self {
        self.deduplicate = deduplicate;
        self
    }
}

impl Query for HostGroupSketchQuery {
    type Collector = GroupSketchCollector;
    type TantivyQuery = Box<dyn tantivy::query::Query>;
    type IntermediateOutput = FxHashMap<u128, HyperLogLog<4096>>;
    type Output = FxHashMap<u128, HyperLogLog<4096>>;

    fn tantivy_query(&self, searcher: &crate::webgraph::searcher::Searcher) -> Self::TantivyQuery {
        let mut raw: Self::TantivyQuery = match self.node {
            LinksDirection::From(node) => {
                let mut q =
                    raw::LinksQuery::new(node, FromHostId, searcher.warmed_column_fields().clone())
                        .skip_self_links(self.skip_self_links);

                if self.deduplicate {
                    q = q.with_deduplication_field(ToHostId)
                }

                Box::new(q)
            }
            LinksDirection::To(node) => {
                let mut q =
                    raw::LinksQuery::new(node, ToHostId, searcher.warmed_column_fields().clone())
                        .skip_self_links(self.skip_self_links);

                if self.deduplicate {
                    q = q.with_deduplication_field(FromHostId)
                }

                Box::new(q)
            }
        };

        if let Some(filter) = self.filter_as_and().and_then(|f| f.inverted_index_filter()) {
            let filter = filter.query(searcher);
            let mut queries = vec![(Occur::Must, raw)];
            queries.extend(filter);
            raw = Box::new(BooleanQuery::new(queries));
        }

        raw
    }

    fn collector(&self, searcher: &crate::webgraph::searcher::Searcher) -> Self::Collector {
        let mut collector = GroupSketchCollector::new(self.group, self.value)
            .with_column_fields(searcher.warmed_column_fields().clone());

        if let Some(filter) = self.filter_as_and().and_then(|f| f.column_field_filter()) {
            collector = collector.with_filter(filter);
        }

        collector
    }

    fn coordinator_collector(&self) -> Self::Collector {
        GroupSketchCollector::new(self.group, self.value)
    }

    fn filter_fruit_shards(
        &self,
        _: crate::ampc::dht::ShardId,
        fruit: <Self::Collector as super::Collector>::Fruit,
    ) -> <Self::Collector as super::Collector>::Fruit {
        fruit
    }

    fn retrieve(
        &self,
        _: &crate::webgraph::searcher::Searcher,
        fruit: <Self::Collector as super::Collector>::Fruit,
    ) -> crate::Result<Self::IntermediateOutput> {
        Ok(fruit)
    }

    fn merge_results(mut results: Vec<Self::IntermediateOutput>) -> Self::Output {
        results.pop().unwrap_or_default()
    }
}

#[derive(Debug, Clone, bincode::Encode, bincode::Decode)]
pub struct HostGroupQuery {
    node: LinksDirection,
    group: FieldEnum,
    value: FieldEnum,
    filters: Vec<FilterEnum>,
    skip_self_links: bool,
    deduplicate: bool,
}

impl HostGroupQuery {
    pub fn new<Group: Field, Value: Field>(
        node: LinksDirection,
        group: Group,
        value: Value,
    ) -> Self {
        Self {
            node,
            group: group.into(),
            value: value.into(),
            filters: Vec::new(),
            skip_self_links: true,
            deduplicate: true,
        }
    }

    pub fn backlinks<Group: Field, Value: Field>(node: NodeID, group: Group, value: Value) -> Self {
        Self::new(LinksDirection::To(node), group, value)
    }

    pub fn forwardlinks<Group: Field, Value: Field>(
        node: NodeID,
        group: Group,
        value: Value,
    ) -> Self {
        Self::new(LinksDirection::From(node), group, value)
    }

    #[must_use]
    pub fn filter<F: Filter>(mut self, filter: F) -> Self {
        self.filters.push(filter.into());
        self
    }

    fn filter_as_and(&self) -> Option<AndFilter> {
        if self.filters.is_empty() {
            None
        } else {
            let mut filter = AndFilter::new();

            for f in self.filters.clone() {
                filter = filter.and(f);
            }

            Some(filter)
        }
    }

    #[must_use]
    pub fn skip_self_links(mut self, skip_self_links: bool) -> Self {
        self.skip_self_links = skip_self_links;
        self
    }

    #[must_use]
    pub fn deduplicate(mut self, deduplicate: bool) -> Self {
        self.deduplicate = deduplicate;
        self
    }
}

impl Query for HostGroupQuery {
    type Collector = GroupExactCollector;
    type TantivyQuery = Box<dyn tantivy::query::Query>;
    type IntermediateOutput = FxHashMap<u128, FxHashSet<u128>>;
    type Output = FxHashMap<u128, FxHashSet<u128>>;

    fn tantivy_query(&self, searcher: &crate::webgraph::searcher::Searcher) -> Self::TantivyQuery {
        let mut raw: Self::TantivyQuery = match self.node {
            LinksDirection::From(node) => {
                let mut q =
                    raw::LinksQuery::new(node, FromHostId, searcher.warmed_column_fields().clone())
                        .skip_self_links(self.skip_self_links);

                if self.deduplicate {
                    q = q.with_deduplication_field(ToHostId)
                }

                Box::new(q)
            }
            LinksDirection::To(node) => {
                let mut q =
                    raw::LinksQuery::new(node, ToHostId, searcher.warmed_column_fields().clone())
                        .skip_self_links(self.skip_self_links);

                if self.deduplicate {
                    q = q.with_deduplication_field(FromHostId)
                }

                Box::new(q)
            }
        };

        if let Some(filter) = self.filter_as_and().and_then(|f| f.inverted_index_filter()) {
            let filter = filter.query(searcher);
            let mut queries = vec![(Occur::Must, raw)];
            queries.extend(filter);
            raw = Box::new(BooleanQuery::new(queries));
        }

        raw
    }

    fn collector(&self, searcher: &crate::webgraph::searcher::Searcher) -> Self::Collector {
        let mut collector = GroupExactCollector::new(self.group, self.value)
            .with_column_fields(searcher.warmed_column_fields().clone());

        if let Some(filter) = self.filter_as_and().and_then(|f| f.column_field_filter()) {
            collector = collector.with_filter(filter);
        }

        collector
    }

    fn coordinator_collector(&self) -> Self::Collector {
        GroupExactCollector::new(self.group, self.value)
    }

    fn filter_fruit_shards(
        &self,
        _: crate::ampc::dht::ShardId,
        fruit: <Self::Collector as super::Collector>::Fruit,
    ) -> <Self::Collector as super::Collector>::Fruit {
        fruit
    }

    fn retrieve(
        &self,
        _: &crate::webgraph::searcher::Searcher,
        fruit: <Self::Collector as super::Collector>::Fruit,
    ) -> crate::Result<Self::IntermediateOutput> {
        Ok(fruit)
    }

    fn merge_results(mut results: Vec<Self::IntermediateOutput>) -> Self::Output {
        results.pop().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use crate::webgraph::{tests::test_graph, Node};

    use super::*;

    #[test]
    fn test_group_sketch_query() {
        let (graph, _temp_dir) = test_graph();

        let id = Node::from("C").into_host().id();
        let query = HostGroupSketchQuery::backlinks(id, ToHostId, FromHostId);
        let result = graph.search(&query).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.get(&id.as_u128()).unwrap().size(), 3);
    }

    #[test]
    fn test_group_exact_query() {
        let (graph, _temp_dir) = test_graph();

        let id = Node::from("C").into_host().id();
        let query = HostGroupQuery::backlinks(id, ToHostId, FromHostId);
        let result = graph.search(&query).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.get(&id.as_u128()).unwrap().len(), 3);
    }
}
