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
// along with this program.  If not, see <https://www.gnu.org/licenses/

use std::{
    cmp,
    collections::{BTreeMap, BinaryHeap},
};

use crate::config::WebgraphGranularity;

use super::{query, EdgeLimit, Node, NodeID, SmallEdge, Webgraph};

pub trait ShortestPaths {
    fn distances(&self, source: Node, granularity: WebgraphGranularity) -> BTreeMap<Node, u8>;
    fn raw_distances(
        &self,
        source: NodeID,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8>;
    fn raw_distances_with_max(
        &self,
        source: NodeID,
        max_dist: u8,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8>;
    fn raw_reversed_distances_with_max(
        &self,
        source: NodeID,
        max_dist: u8,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8>;
    fn raw_reversed_distances(
        &self,
        source: NodeID,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8>;
    fn reversed_distances(
        &self,
        source: Node,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<Node, u8>;
}

fn dijkstra_multi<F1, F2>(
    sources: &[NodeID],
    node_edges: F1,
    edge_node: F2,
    max_dist: Option<u8>,
) -> BTreeMap<NodeID, u8>
where
    F1: Fn(NodeID) -> Vec<SmallEdge>,
    F2: Fn(&SmallEdge) -> NodeID,
{
    let mut distances: BTreeMap<NodeID, u8> = BTreeMap::default();

    let mut queue = BinaryHeap::new();

    for source_id in sources.iter().copied() {
        queue.push(cmp::Reverse((0, source_id)));
        distances.insert(source_id, 0);
    }

    while let Some(state) = queue.pop() {
        let (cost, v) = state.0;

        let current_dist = distances.get(&v).unwrap_or(&u8::MAX);

        if cost > *current_dist {
            continue;
        }

        if let Some(max_dist) = max_dist {
            if cost > max_dist {
                return distances;
            }
        }

        for edge in node_edges(v) {
            if cost + 1 < *distances.get(&edge_node(&edge)).unwrap_or(&u8::MAX) {
                let d = cost + 1;

                let next = cmp::Reverse((d, edge_node(&edge)));
                queue.push(next);
                distances.insert(edge_node(&edge), d);
            }
        }
    }

    distances
}

impl ShortestPaths for Webgraph {
    fn distances(&self, source: Node, granularity: WebgraphGranularity) -> BTreeMap<Node, u8> {
        self.raw_distances(source.id(), granularity)
            .into_iter()
            .filter_map(|(id, dist)| match granularity {
                WebgraphGranularity::Host => self
                    .search(&query::Id2NodeQuery::Host(id))
                    .unwrap()
                    .map(|node| (node, dist)),
                WebgraphGranularity::Page => self
                    .search(&query::Id2NodeQuery::Page(id))
                    .unwrap()
                    .map(|node| (node, dist)),
            })
            .collect()
    }

    fn raw_distances_with_max(
        &self,
        source: NodeID,
        max_dist: u8,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8> {
        dijkstra_multi(
            &[source],
            |node| match granularity {
                WebgraphGranularity::Host => self
                    .search(
                        &query::HostForwardlinksQuery::new(node).with_limit(EdgeLimit::Unlimited),
                    )
                    .unwrap_or_default(),
                WebgraphGranularity::Page => self
                    .search(&query::ForwardlinksQuery::new(node).with_limit(EdgeLimit::Unlimited))
                    .unwrap_or_default(),
            },
            |edge| edge.to,
            Some(max_dist),
        )
    }

    fn raw_distances(
        &self,
        source: NodeID,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8> {
        dijkstra_multi(
            &[source],
            |node| match granularity {
                WebgraphGranularity::Host => self
                    .search(
                        &query::HostForwardlinksQuery::new(node).with_limit(EdgeLimit::Unlimited),
                    )
                    .unwrap_or_default(),
                WebgraphGranularity::Page => self
                    .search(&query::ForwardlinksQuery::new(node).with_limit(EdgeLimit::Unlimited))
                    .unwrap_or_default(),
            },
            |edge| edge.to,
            None,
        )
    }

    fn raw_reversed_distances(
        &self,
        source: NodeID,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8> {
        dijkstra_multi(
            &[source],
            |node| match granularity {
                WebgraphGranularity::Host => self
                    .search(&query::HostBacklinksQuery::new(node).with_limit(EdgeLimit::Unlimited))
                    .unwrap_or_default(),
                WebgraphGranularity::Page => self
                    .search(&query::BacklinksQuery::new(node).with_limit(EdgeLimit::Unlimited))
                    .unwrap_or_default(),
            },
            |edge| edge.from,
            None,
        )
    }

    fn raw_reversed_distances_with_max(
        &self,
        source: NodeID,
        max_dist: u8,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<NodeID, u8> {
        dijkstra_multi(
            &[source],
            |node| match granularity {
                WebgraphGranularity::Host => self
                    .search(&query::HostBacklinksQuery::new(node).with_limit(EdgeLimit::Unlimited))
                    .unwrap_or_default(),
                WebgraphGranularity::Page => self
                    .search(&query::BacklinksQuery::new(node).with_limit(EdgeLimit::Unlimited))
                    .unwrap_or_default(),
            },
            |edge| edge.from,
            Some(max_dist),
        )
    }

    fn reversed_distances(
        &self,
        source: Node,
        granularity: WebgraphGranularity,
    ) -> BTreeMap<Node, u8> {
        self.raw_reversed_distances(source.id(), granularity)
            .into_iter()
            .filter_map(|(id, dist)| match granularity {
                WebgraphGranularity::Host => self
                    .search(&query::Id2NodeQuery::Host(id))
                    .unwrap()
                    .map(|node| (node, dist)),
                WebgraphGranularity::Page => self
                    .search(&query::Id2NodeQuery::Page(id))
                    .unwrap()
                    .map(|node| (node, dist)),
            })
            .collect()
    }
}
