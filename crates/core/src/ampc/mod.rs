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

//! # Framework for Adaptive Massively Parallel Computation (AMPC).
//!
//! AMPC is a system for implementing large-scale distributed graph algorithms efficiently.
//! It provides a framework for parallel computation across clusters of machines.
//!
//! While similar in concept to MapReduce, AMPC uses a distributed hash table (DHT) as its
//! underlying data structure rather than the traditional map and reduce phases. This key
//! architectural difference enables more flexible and efficient computation patterns.
//!
//! The main advantage over MapReduce is that workers can dynamically access any keys in
//! the DHT during computation. This is in contrast to MapReduce where the keyspace must
//! be statically partitioned between reducers before computation begins. The dynamic
//! access pattern allows for more natural expression of graph algorithms in a distributed
//! setting.
//!
//! This is roughly inspired by
//! [Massively Parallel Graph Computation: From Theory to Practice](https://research.google/blog/massively-parallel-graph-computation-from-theory-to-practice/)
//!
//! ## Key concepts
//!
//! * **DHT**: A distributed hash table is used to store the result of the computation for
//!     each round.
//! * **Worker**: A worker owns a subset of the overall graph and is responsible for
//!     executing mappers on its portion of the graph and sending results to the DHT.
//! * **Mapper**: A mapper is the specific computation to be run on the graph.
//! * **Coordinator**: The coordinator is responsible for scheduling the jobs on the workers.

use self::{job::Job, worker::WorkerRef};
use crate::distributed::sonic;

mod coordinator;
pub mod dht;
pub mod dht_conn;
mod finisher;
mod job;
mod mapper;
pub mod prelude;
mod server;
mod setup;
mod worker;

use self::prelude::*;

pub use coordinator::Coordinator;
pub use dht_conn::{DefaultDhtTable, DhtConn, DhtTable, DhtTables, Table};
pub use server::Server;
pub use worker::{Message, RequestWrapper, Worker};

#[derive(serde::Serialize, serde::Deserialize, bincode::Encode, bincode::Decode, Clone)]
pub enum CoordReq<J, M, T> {
    CurrentJob,
    ScheduleJob { job: J, mapper: M },
    Setup { dht: DhtConn<T> },
}

#[derive(serde::Serialize, serde::Deserialize, bincode::Encode, bincode::Decode)]
pub enum CoordResp<J> {
    CurrentJob(Option<J>),
    ScheduleJob(()),
    Setup(()),
}

#[derive(serde::Serialize, serde::Deserialize, bincode::Encode, bincode::Decode, Clone)]
pub enum Req<J, M, R, T> {
    Coordinator(CoordReq<J, M, T>),
    User(R),
}

type JobReq<J> =
    Req<J, <J as Job>::Mapper, <<J as Job>::Worker as Worker>::Request, <J as Job>::DhtTables>;

type JobResp<J> = Resp<J, <<J as Job>::Worker as Worker>::Response>;

#[derive(serde::Serialize, serde::Deserialize, bincode::Encode, bincode::Decode)]
pub enum Resp<J, R> {
    Coordinator(CoordResp<J>),
    User(R),
}

type JobDht<J> = DhtConn<<J as Job>::DhtTables>;

pub type JobConn<J> = sonic::Connection<JobReq<J>, JobResp<J>>;

#[must_use = "this `JobScheduled` may not have scheduled the job on any worker"]
enum JobScheduled {
    Success(WorkerRef),
    NoAvailableWorkers,
}
