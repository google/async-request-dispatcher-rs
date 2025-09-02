// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # Async Request Dispatcher (Worker Pool)
//!
//! An asynchronous **in-process request dispatcher** built on top of [`tokio`].
//!
//! This crate provides a pool of workers for handling jobs concurrently, with support for:
//!
//! - **Least-loaded worker selection**
//! - **Session affinity** with TTL
//! - **Worker-level metrics** (in-flight jobs, queue usage, errors)
//! - **Configurable concurrency limits** per worker
//!
//! ## Core Concepts
//!
//! - A [`Job<R, S>`] represents a request of type `R` and a oneshot channel to return a response of type `S`.
//! - [`WorkerHandle<R, S>`] is a lightweight handle to an async worker with its own queue.
//! - [`WorkerPool<R, S>`] manages a set of workers and routes jobs using least-loaded + affinity logic.
//!
//! ## Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//! use tokio;
//! use async_request_dispatcher::worker_pool::{WorkerPool, spawn_workers};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Spawn workers with a simple handler
//!     let workers = spawn_workers(4, |req: String| async move {
//!         Ok::<_, anyhow::Error>(format!("echo: {}", req))
//!     });
//!
//!     // Create worker pool with 30s session affinity TTL
//!     let wp = WorkerPool::new(workers, Duration::from_secs(30));
//!
//!     // Submit a job (oneshot request/response)
//!     let response = wp.submit(Some("session-1"), "hello".to_string()).await?;
//!
//!     println!("Response: {}", response);
//!     Ok(())
//! }
//! ```
//!
//! ## When to Use
//!
//! - Building **async APIs** that need to fan out requests to multiple worker tasks
//! - Adding **fair scheduling** and **affinity** on top of raw [`tokio::mpsc`] channels
//! - Collecting per-worker **metrics** for observability
//!
//! ## Limitations
//!
//! - Only **oneshot request/response** is supported today (`R -> S`).
//! - Streaming handlers (e.g., `R -> Stream<Item = S>`) are not yet implemented.
//!
//! ## License
//!
//! Licensed under [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0).
mod constants;
pub mod worker_pool;
