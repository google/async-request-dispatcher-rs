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

use crate::constants::{MAX_CONCURRENCY_PER_WORKER, MAX_WORKER_QUEUE_CAPACITY};
use dashmap::DashMap;
use log::{debug, error};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub struct Job<R, S> {
    pub req: R,
    pub respond_to: oneshot::Sender<S>,
}

#[derive(Clone)]
pub struct WorkerHandle<R, S> {
    pub id: usize,
    pub tx: mpsc::Sender<Job<R, S>>,
    pub inflight: Arc<AtomicUsize>,
    pub errors: Arc<AtomicUsize>,
}

#[derive(Serialize)]
pub struct WorkerMetrics {
    pub worker_id: usize,
    pub inflight: usize,
    pub queue_len: usize,
    pub capacity: usize,
    pub errors: usize,
}

impl<R, S> WorkerHandle<R, S> {
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::Relaxed)
    }

    pub fn metrics(&self) -> WorkerMetrics {
        WorkerMetrics {
            worker_id: self.id,
            inflight: self.inflight(),
            queue_len: self.tx.max_capacity() - self.tx.capacity(),
            capacity: self.tx.max_capacity(),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

/// Worker pool with least-conn + affinity
pub struct WorkerPool<R, S> {
    workers: Vec<WorkerHandle<R, S>>,
    affinity: DashMap<String, (usize, Instant)>,
    affinity_ttl: Duration,
}

impl<R, S> WorkerPool<R, S>
where
    R: Send + 'static,
    S: Send + 'static,
{
    pub fn new(workers: Vec<WorkerHandle<R, S>>, affinity_ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            workers,
            affinity: DashMap::new(),
            affinity_ttl,
        })
    }

    fn choose_least_loaded_worker(&self) -> usize {
        self.workers
            .iter()
            .enumerate()
            .min_by_key(|(_, w)| w.inflight() + (w.tx.max_capacity() - w.tx.capacity()))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn choose_worker_idx(&self, session: Option<&str>) -> usize {
        let now = Instant::now();
        self.affinity.retain(|_, v| v.1 > now);

        if let Some(sid) = session {
            if let Some(entry) = self.affinity.get(sid) {
                if entry.value().1 > now {
                    debug!(
                        "session {} hits affinity for worker {}",
                        sid,
                        entry.value().0
                    );
                    return entry.value().0;
                }
            }
        }

        let idx = self.choose_least_loaded_worker();
        debug!(
            "worker {} selected, current inflight {}",
            idx,
            self.workers[idx].inflight()
        );
        if let Some(sid) = session {
            self.affinity
                .insert(sid.to_string(), (idx, now + self.affinity_ttl));
        }
        idx
    }

    pub async fn submit(self: &Arc<Self>, session: Option<&str>, req: R) -> anyhow::Result<S> {
        let idx = self.choose_worker_idx(session);
        let worker = &self.workers[idx];

        let (tx, rx) = oneshot::channel();
        let job = Job {
            req,
            respond_to: tx,
        };

        if let Err(e) = worker.tx.send(job).await {
            worker.errors.fetch_add(1, Ordering::Relaxed);
            return Err(anyhow::anyhow!("send failed: {e}"));
        }

        let resp = rx.await?;
        Ok(resp)
    }

    pub fn worker_metrics(&self) -> Vec<WorkerMetrics> {
        self.workers.iter().map(|w| w.metrics()).collect()
    }
}

/// Spawn `n` workers, each runs `handler(req) -> Future<Resp>`
pub fn spawn_workers<R, S, F, Fut, E>(n: usize, handler: F) -> Vec<WorkerHandle<R, S>>
where
    R: Send + 'static,
    S: Send + 'static,
    F: Fn(R) -> Fut + Send + Clone + 'static,
    Fut: Future<Output = Result<S, E>> + Send + 'static,
    E: Send + std::fmt::Debug + 'static,
{
    (0..n)
        .map(|id| {
            let (tx, mut rx) = mpsc::channel::<Job<R, S>>(MAX_WORKER_QUEUE_CAPACITY);
            let inflight = Arc::new(AtomicUsize::new(0));
            let inflight_worker = inflight.clone();
            let errors = Arc::new(AtomicUsize::new(0));
            let errors_clone = errors.clone();
            let handler_clone = handler.clone();

            let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENCY_PER_WORKER));

            tokio::spawn({
                let semaphore = semaphore.clone();
                async move {
                    while let Some(Job { req, respond_to }) = rx.recv().await {
                        match semaphore.clone().acquire_owned().await {
                            Ok(permit) => {
                                let inflight = inflight_worker.clone();
                                let handler = handler_clone.clone();
                                let errors = errors_clone.clone();

                                inflight.fetch_add(1, Ordering::Relaxed);
                                tokio::spawn(async move {
                                    match handler(req).await {
                                        Ok(resp) => {
                                            let _ = respond_to.send(resp);
                                        }
                                        Err(e) => {
                                            error!(
                                                "worker {} error processing request: {:?}",
                                                id, e
                                            );
                                            errors.fetch_add(1, Ordering::Relaxed);
                                        }
                                    };
                                    inflight.fetch_sub(1, Ordering::Relaxed);
                                    drop(permit);
                                });
                            }
                            Err(_) => {
                                inflight_worker.fetch_sub(1, Ordering::Relaxed);
                                errors_clone.fetch_add(1, Ordering::Relaxed);
                                debug!("worker {} semaphore closed, dropping job", id);
                            }
                        }
                    }
                }
            });

            WorkerHandle {
                id,
                tx,
                inflight,
                errors,
            }
        })
        .collect()
}
