
---

## Async Request Dispatcher with Least-Loaded Worker Selection + Affinity

This Rust project implements a high-performance **in-process asynchronous request dispatcher** using `tokio`. It distributes requests across a pool of workers with support for **least-loaded worker selection** and **session affinity**.

### Included example: GCP GenAI API Request Dispatcher

```shell
cargo run --example gcp-genai-api
```

---

### Features

- **Async request handling:** Uses `tokio` tasks and channels to process jobs concurrently.
- **Least-loaded worker selection:** Requests are routed to the worker with the lowest total load (active + queued jobs).
- **Session affinity:** Optionally routes requests from the same session to the same worker within a configurable TTL.
- **Worker metrics:** Tracks in-flight jobs, queue length, capacity, and errors for observability.
- **Configurable concurrency:** Each worker has a semaphore-limited concurrency to prevent overload.

---

### Architecture

```text
Client/API
    │
    ▼
+----------------------+
|   Worker Pool       |
|  +----------------+ |
|  | Session Map     | |
|  | session -> idx  | |
|  +----------------+ |
+----------------------+
    │
    ▼ Least-Loaded Worker Selection
    │
+----+----+----+----+
| W0 | W1 | W2 | WN |
+----+----+----+----+
  │     │     │
  ▼     ▼     ▼
Queue  Queue Queue ...
  │     │     │
In-flight/In-flight/In-flight
Errors/Errors/Errors
```

### Core Components

#### Job

A Job represents a request sent to a worker along with a channel to respond asynchronously:

```rust
pub struct Job<R, S> {
    pub req: R,
    pub respond_to: oneshot::Sender<S>,
}
```

#### WorkerHandle

A worker is responsible for processing jobs asynchronously. It includes:

- **Job queue** (MPSC channel)
- **In-flight counter** (`AtomicUsize`)
- **Error counter** (`AtomicUsize`)

```rust
pub struct WorkerHandle<R, S> {
    pub id: usize,
    pub tx: mpsc::Sender<Job<R, S>>,
    pub inflight: Arc<AtomicUsize>,
    pub errors: Arc<AtomicUsize>,
}
```

Workers are spawned with the helper function:

```rust
pub fn spawn_workers<R, S, F, Fut, E>(n: usize, handler: F) -> Vec<WorkerHandle<R, S>>
where
    F: Fn(R) -> Fut + Send + Clone + 'static,
    Fut: Future<Output = Result<S, E>> + Send + 'static,
    E: Send + std::fmt::Debug + 'static,
```

This function creates `n` workers, each executing the provided handler function with a concurrency limit.

---

#### WorkerPool

The `WorkerPool` manages worker selection and job submission.

```rust
pub struct WorkerPool<R, S> {
    workers: Vec<WorkerHandle<R, S>>,
    affinity: DashMap<String, (usize, Instant)>,
    affinity_ttl: Duration,
}
```

**Worker selection logic:**

- **Least-loaded:** Picks the worker with the lowest in-flight + queued count.
- **Session affinity:** Routes requests for a session to the same worker if within TTL.

**Submitting a job:**

```rust
let response: S = wp.submit(Some("session-id"), request).await?;
```

**Getting metrics:**

```rust
let metrics = wp.worker_metrics();
```

---

### Usage Example

```rust
use axum::{routing::post, Router};
use std::sync::Arc;
use tokio;

#[tokio::main]
async fn main() {
    let workers = spawn_workers(num_cpus::get(), move |req| {
        async move {
            // Handle request asynchronously
            process_request(req).await
        }
    });

    let wp = WorkerPool::new(workers, std::time::Duration::from_secs(30));

    let app = Router::new().route("/submit", post({
        let wp = wp.clone();
        move |req| async move {
            lb.submit(Some("session-id"), req).await
        }
    }));

    axum::Server::bind(&"0.0.0.0:8080".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
```

---

### Metrics

Each worker reports:

- `worker_id` – Unique worker identifier
- `inflight` – Number of currently processing jobs
- `queue_len` – Jobs waiting in the worker's queue
- `capacity` – Maximum queue capacity
- `errors` – Number of failed or dropped jobs

---

### Configuration Constants

```rust
MAX_CONCURRENCY_PER_WORKER   // Maximum concurrent jobs per worker
MAX_WORKER_QUEUE_CAPACITY    // Maximum queue length per worker
```

Adjust these values to tune throughput and backpressure behavior.

---

### Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for details.

---

### License

Apache 2.0. See [`LICENSE`](LICENSE) for details.

---

### Disclaimer

This is not an officially supported Google product. This project is not eligible for the [Google Open Source Software Vulnerability Rewards Program](https://bughunters.google.com/open-source-security).

---
