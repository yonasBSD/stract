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

pub mod embedding;
pub mod inbound_similarity;
pub mod recall;
pub mod reranker;

pub use recall::Recall;
pub use reranker::ReRanker;

use crate::searcher::SearchQuery;

use super::RankableWebpage;

pub trait Scorer<T: RankableWebpage>: Send + Sync {
    fn score(&self, webpages: &mut [T]);
    fn set_query_info(&mut self, _query: &SearchQuery) {}
}

pub struct IdentityScorer;

impl<T: RankableWebpage> Scorer<T> for IdentityScorer {
    fn score(&self, _webpages: &mut [T]) {}
}

pub struct MultiScorer<T: RankableWebpage> {
    scorers: Vec<Box<dyn Scorer<T>>>,
}

impl<T: RankableWebpage> MultiScorer<T> {
    pub fn new(scorers: Vec<Box<dyn Scorer<T>>>) -> Self {
        Self { scorers }
    }
}

impl<T: RankableWebpage> Scorer<T> for MultiScorer<T> {
    fn score(&self, webpages: &mut [T]) {
        for scorer in &self.scorers {
            scorer.score(webpages);
        }
    }

    fn set_query_info(&mut self, query: &SearchQuery) {
        for scorer in &mut self.scorers {
            scorer.set_query_info(query);
        }
    }
}
