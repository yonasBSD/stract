// Cuely is an open source web search engine.
// Copyright (C) 2022 Cuely ApS
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

use std::net::SocketAddr;

use crate::{
    bangs::Bangs,
    entity_index::EntityIndex,
    index::Index,
    inverted_index,
    ranking::centrality_store::CentralityStore,
    search_prettifier::{self},
    searcher::{self, LocalSearcher},
    sonic, Result, SearchServerConfig,
};

pub async fn run(config: SearchServerConfig) -> Result<()> {
    let addr: SocketAddr = config.host.parse().unwrap();
    let server = sonic::Server::bind(addr).await.unwrap();
    tracing::info!("listening on {}", addr);

    let entity_index = config
        .entity_index_path
        .map(|path| EntityIndex::open(path).unwrap());
    let bangs = config.bangs_path.map(Bangs::from_path);
    let centrality_store = config.centrality_store_path.map(CentralityStore::open);
    let search_index = Index::open(config.index_path)?;

    let mut local_searcher = LocalSearcher::new(search_index);

    if let Some(entity_index) = entity_index {
        local_searcher.set_entity_index(entity_index);
    }

    if let Some(bangs) = bangs {
        local_searcher.set_bangs(bangs);
    }

    if let Some(centrality_store) = centrality_store {
        local_searcher.set_centrality_store(centrality_store);
    }

    loop {
        if let Ok(req) = server.accept::<searcher::distributed::Request>().await {
            match &req.body {
                searcher::Request::Search(query) => {
                    match local_searcher.search_initial(query, false) {
                        Ok(response) => {
                            req.respond(sonic::Response::Content(response)).await.ok();
                        }
                        Err(_) => {
                            req.respond::<searcher::InitialSearchResult>(sonic::Response::Empty)
                                .await
                                .ok();
                        }
                    }
                }
                searcher::Request::RetrieveWebsites { websites, query } => {
                    match local_searcher.retrieve_websites(websites, query) {
                        Ok(response) => {
                            req.respond(sonic::Response::Content(response)).await.ok();
                        }
                        Err(_) => {
                            req.respond::<Vec<inverted_index::RetrievedWebpage>>(
                                sonic::Response::Empty,
                            )
                            .await
                            .ok();
                        }
                    }
                }
                searcher::Request::SearchPrettified(query) => {
                    match local_searcher.search_initial(query, false) {
                        Ok(result) => match result {
                            searcher::InitialSearchResult::Websites(result) => {
                                let res = search_prettifier::initial(result);

                                req.respond(sonic::Response::Content(
                                    searcher::InitialPrettifiedSearchResult::Websites(res),
                                ))
                                .await
                                .ok();
                            }
                            searcher::InitialSearchResult::Bang(bang) => {
                                req.respond(sonic::Response::Content(
                                    searcher::InitialSearchResult::Bang(bang),
                                ))
                                .await
                                .ok();
                            }
                        },
                        Err(_) => {
                            req.respond::<inverted_index::SearchResult>(sonic::Response::Empty)
                                .await
                                .ok();
                        }
                    }
                }
            }
        }
    }
}
