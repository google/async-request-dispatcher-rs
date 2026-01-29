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

use crate::worker_pool::{WorkerMetrics, WorkerPool};
use anyhow::Context;
use async_request_dispatcher::worker_pool;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use dashmap::DashMap;
use env_logger::Env;
use firestore::{FirestoreDb, FirestoreDbOptions};
use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_aiplatform_v1::model::{GenerateContentRequest, GenerateContentResponse};
use google_cloud_auth::credentials::{Builder, Credentials, impersonated};
use google_cloud_gax::exponential_backoff::ExponentialBackoffBuilder;
use google_cloud_gax::options::RequestOptionsBuilder;
use google_cloud_gax::retry_policy::{Aip194Strict, RetryPolicyExt};
use google_cloud_gax::retry_throttler::AdaptiveThrottler;
use http::HeaderMap;
use http::StatusCode;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::env::var;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    let num_cpus = std::thread::available_parallelism()?;
    info!("number of CPUs available: {}", num_cpus);

    let credentials = ApiCredentials::build()?;

    let project_id = var("GOOGLE_CLOUD_PROJECT")?;
    let model_id = var("MODEL_ID")?;

    let endpoint = "https://aiplatform.googleapis.com";
    let exponential_backoff = ExponentialBackoffBuilder::new()
        .with_initial_delay(Duration::from_secs(2))
        .with_maximum_delay(Duration::from_secs(60))
        .with_scaling(2.0)
        .build()?;
    let retry_policy = Aip194Strict.with_attempt_limit(5);
    let prediction_service = PredictionService::builder()
        .with_credentials(credentials.inner.clone())
        .with_endpoint(endpoint)
        .with_backoff_policy(exponential_backoff)
        .with_retry_policy(retry_policy)
        .with_retry_throttler(AdaptiveThrottler::default())
        .build()
        .await?;

    let workers = worker_pool::spawn_workers::<ApiRequest, GenerateContentResponse, _, _, _>(
        num_cpus.into(),
        move |r| {
            let svc = prediction_service.clone();
            async move { process_request(r, svc).await }
        },
    );

    let wp = WorkerPool::new(workers, Duration::from_secs(300));
    let (firestore_db, firestore_collection) = match var("FIRESTORE_COLLECTION") {
        Ok(col) => (
            Some(
                FirestoreDb::with_options(
                    FirestoreDbOptions::new(var("GOOGLE_CLOUD_PROJECT")?).with_database_id(
                        var("FIRESTORE_DATABASE")
                            .with_context(|| "FIRESTORE_DATABASE env variable is required.")?,
                    ),
                )
                .await?,
            ),
            col,
        ),
        Err(_) => (None, env!("CARGO_BIN_NAME").to_string()),
    };

    let job_registry = Arc::new(DashMap::new());
    let registry_clone = job_registry.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            let now = Instant::now();
            let ttl = Duration::from_secs(3600);
            let initial_len = registry_clone.len();
            registry_clone.retain(|_, entry: &mut JobEntry<GenerateContentResponse>| {
                now.duration_since(entry.created_at) < ttl
            });
            let removed = initial_len - registry_clone.len();
            if removed > 0 {
                info!("cleaning up {} expired jobs", removed);
            }
        }
    });

    let app = Router::new()
        .route("/generate", post(generate_content))
        .route("/jobs/{id}", get(get_job_status))
        .route("/metrics", get(worker_metrics))
        .with_state(Api {
            wp,
            project_id: Arc::new(project_id),
            model_id: Arc::new(model_id),
            job_registry,
            firestore_db,
            firestore_collection: Arc::new(firestore_collection),
        });

    let addr = "0.0.0.0:8080";
    info!("request dispatcher listening: {}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
pub(crate) struct Api {
    pub(crate) wp: Arc<WorkerPool<ApiRequest, GenerateContentResponse>>,
    pub(crate) project_id: Arc<String>,
    pub(crate) model_id: Arc<String>,
    pub(crate) job_registry: Arc<DashMap<String, JobEntry<GenerateContentResponse>>>,
    pub(crate) firestore_db: Option<FirestoreDb>,
    pub(crate) firestore_collection: Arc<String>,
}

#[derive(Debug)]
pub struct ApiRequest {
    pub req: GenerateContentRequest,
}

use axum::response::IntoResponse;

#[derive(thiserror::Error, Debug)]
pub enum ApiError {
    #[error("job: {0} not found")]
    NotFound(String),

    #[error("firestore error: {0}")]
    FirestoreError(String),

    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, format!("error occurred: {}", self)).into_response()
    }
}

pub(crate) type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "status", content = "result")]
pub enum JobStatus<T> {
    Pending,
    Completed(T),
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JobEntry<T> {
    pub status: JobStatus<T>,
    #[serde(skip, default = "Instant::now")]
    pub created_at: Instant,
}

#[derive(Serialize)]
pub struct ApiResponse {
    pub job_id: String,
}

pub(crate) async fn generate_content(
    State(Api {
        wp,
        project_id,
        model_id,
        job_registry,
        firestore_db,
        firestore_collection,
    }): State<Api>,
    headers: HeaderMap,
    mut req: Json<GenerateContentRequest>,
) -> ApiResult<Json<ApiResponse>> {
    let session = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    req.0.model = format!(
        "projects/{}/locations/global/publishers/google/models/{}",
        project_id, model_id
    );
    debug!("processing the request using model: {}", &req.0.model);

    let job_id = Uuid::new_v4().to_string();
    let entry = JobEntry {
        status: JobStatus::Pending,
        created_at: Instant::now(),
    };
    job_registry.insert(job_id.clone(), entry.clone());

    if let Some(db) = firestore_db.clone() {
        tokio::time::timeout(
            Duration::from_secs(600),
            db.fluent()
                .update()
                .in_col(&firestore_collection)
                .document_id(&job_id)
                .object(&entry)
                .execute::<()>(),
        )
        .await
        .map_err(|_| ApiError::FirestoreError(format!("job: {} timed out", job_id)))?
        .map_err(|e| ApiError::FirestoreError(format!("job: {} error occurred: {}", job_id, e)))?;
    }

    let wp = wp.clone();
    let registry = job_registry.clone();
    let id_clone = job_id.clone();
    let generate_content_req = req.0;
    let session_clone = session.clone();
    let db_clone = firestore_db.clone();
    let collection_clone = firestore_collection.clone();

    tokio::spawn(async move {
        let result = wp
            .submit(
                session_clone.as_deref(),
                ApiRequest {
                    req: generate_content_req.clone(),
                },
            )
            .await;

        let final_entry = match result {
            Ok(res) => JobEntry {
                status: JobStatus::Completed(res),
                created_at: Instant::now(),
            },
            Err(e) => {
                error!(
                    "error occurred processing request: {:?}",
                    serde_json::to_string(&generate_content_req)
                );
                JobEntry {
                    status: JobStatus::Failed(e.to_string()),
                    created_at: Instant::now(),
                }
            }
        };

        registry.insert(id_clone.clone(), final_entry.clone());

        if let Some(db) = db_clone {
            tokio::time::timeout(
                Duration::from_secs(600),
                db.fluent()
                    .update()
                    .in_col(&collection_clone)
                    .document_id(&id_clone)
                    .object(&final_entry)
                    .execute::<()>(),
            )
            .await
            .map_err(|_| {
                error!(
                    "{}",
                    ApiError::FirestoreError(format!("job: {} timed out", id_clone))
                )
            })
            .map(|res| {
                if let Err(e) = res {
                    error!(
                        "{}",
                        ApiError::FirestoreError(format!(
                            "job: {} error occurred: {}",
                            id_clone, e
                        ))
                    );
                }
            })
            .ok();
        }
    });

    Ok(Json(ApiResponse { job_id }))
}

pub(crate) async fn get_job_status(
    State(Api {
        job_registry,
        firestore_db,
        firestore_collection,
        ..
    }): State<Api>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobStatus<GenerateContentResponse>>> {
    match job_registry.get(&id) {
        Some(entry) => Ok(Json(entry.value().status.clone())),
        None => {
            if let Some(db) = firestore_db {
                let val: Option<JobEntry<GenerateContentResponse>> = tokio::time::timeout(
                    Duration::from_secs(600),
                    db.fluent()
                        .select()
                        .by_id_in(&firestore_collection)
                        .obj()
                        .one(&id),
                )
                .await
                .map_err(|_| ApiError::FirestoreError(format!("job: {} timed out", id)))?
                .map_err(|e| {
                    ApiError::FirestoreError(format!("job: {} error occurred: {}", id, e))
                })?;

                if let Some(entry) = val {
                    return Ok(Json(entry.status));
                }
            }
            Err(ApiError::NotFound(id))
        }
    }
}

pub(crate) async fn worker_metrics(State(Api { wp, .. }): State<Api>) -> Json<Vec<WorkerMetrics>> {
    Json(wp.worker_metrics())
}

pub(crate) async fn process_request(
    r: ApiRequest,
    svc: PredictionService,
) -> anyhow::Result<GenerateContentResponse> {
    debug!("processing request: {}", serde_json::to_string(&r.req)?);
    let res = svc
        .generate_content()
        .with_request(r.req)
        .with_attempt_timeout(Duration::from_secs(300))
        .send()
        .await?;
    debug!("api response: {:?}", &res);
    Ok(res)
}

#[derive(Clone, Debug)]
pub struct ApiCredentials {
    inner: Credentials,
}

impl ApiCredentials {
    pub fn build() -> anyhow::Result<Self> {
        let source_credentials = Builder::default().build()?;
        let credentials = if let Ok(sa) = var("GOOGLE_CLOUD_SERVICE_ACCOUNT") {
            impersonated::Builder::from_source_credentials(source_credentials)
                .with_target_principal(sa)
                .build()?
        } else {
            source_credentials
        };
        Ok(Self { inner: credentials })
    }
}
