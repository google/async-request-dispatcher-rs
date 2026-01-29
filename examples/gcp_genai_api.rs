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
use anyhow::{Context, bail};
use async_request_dispatcher::worker_pool;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use env_logger::Env;
use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_aiplatform_v1::model::{GenerateContentRequest, GenerateContentResponse};
use google_cloud_auth::credentials::{Builder, Credentials, impersonated};
use google_cloud_gax::exponential_backoff::ExponentialBackoff;
use google_cloud_gax::retry_policy::{AlwaysRetry, RetryPolicyExt};
use http::HeaderMap;
use http::StatusCode;
use log::{debug, info};
use serde::Deserialize;
use std::collections::HashMap;
use std::env::var;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;

static REGION_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    let num_cpus = std::thread::available_parallelism()?;
    info!("number of CPUs available: {}", num_cpus);

    let credentials = ApiCredentials::build()?;
    let creds = credentials.clone();
    let workers = worker_pool::spawn_workers::<ApiRequest, GenerateContentResponse, _, _, _>(
        num_cpus.into(),
        move |r| {
            let creds = creds.clone();
            async move { process_request(r, creds).await }
        },
    );

    let wp = WorkerPool::new(workers, Duration::from_secs(300));
    let project_id = var("GOOGLE_CLOUD_PROJECT")?;
    let model_id = var("MODEL_ID")?;
    let data = var("MODEL_CONFIG").with_context(|| "MODEL_CONFIG env variable is required!")?;
    let model_config: Vec<ModelConfig> = serde_json::from_str(&data)?;
    let model_map: HashMap<String, Vec<String>> = model_config
        .into_iter()
        .map(|c| (c.model_id, c.supported_regions))
        .collect();
    let regions = match model_map.get(&model_id) {
        Some(r) => {
            info!(
                "using the configured {} regions for model_id: {}",
                r.iter().len(),
                model_id
            );
            r.to_owned()
        }
        None => bail!(
            "supported regions data is required for model_id: {}",
            model_id
        ),
    };

    let app = Router::new()
        .route("/generate", post(generate_content))
        .route("/metrics", get(worker_metrics))
        .with_state(Api {
            wp,
            project_id: Arc::new(project_id),
            model_id: Arc::new(model_id),
            supported_regions: Arc::new(regions),
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
    pub(crate) supported_regions: Arc<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ModelConfig {
    model_id: String,
    supported_regions: Vec<String>,
}

#[derive(Debug)]
pub struct ApiRequest {
    pub region: String,
    pub req: GenerateContentRequest,
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

pub(crate) async fn generate_content(
    State(Api {
        wp,
        project_id,
        model_id,
        supported_regions,
    }): State<Api>,
    headers: HeaderMap,
    mut req: Json<GenerateContentRequest>,
) -> ApiResult<Json<GenerateContentResponse>> {
    let session = headers.get("x-session-id").and_then(|v| v.to_str().ok());

    let index = REGION_COUNTER.fetch_add(1, Ordering::SeqCst) % supported_regions.len();
    let region = &supported_regions[index];

    req.0.model = format!(
        "projects/{}/locations/{}/publishers/google/models/{}",
        project_id, region, model_id
    );
    info!("processing the request using model: {}", &req.0.model);

    match wp
        .submit(
            session,
            ApiRequest {
                region: region.clone(),
                req: req.0,
            },
        )
        .await
    {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("error submitting job: {e}"),
        )),
    }
}

pub(crate) async fn worker_metrics(State(Api { wp, .. }): State<Api>) -> Json<Vec<WorkerMetrics>> {
    Json(wp.worker_metrics())
}

pub(crate) async fn process_request(
    r: ApiRequest,
    creds: ApiCredentials,
) -> anyhow::Result<GenerateContentResponse> {
    info!("processing request: {}", serde_json::to_string(&r.req)?);
    let credentials = creds.inner;
    let endpoint = format!("https://{}-aiplatform.googleapis.com", r.region);
    let exponential_backoff = ExponentialBackoff::default();
    let retry_policy = AlwaysRetry.with_attempt_limit(3);
    let svc = PredictionService::builder()
        .with_credentials(credentials)
        .with_endpoint(endpoint)
        .with_backoff_policy(exponential_backoff)
        .with_retry_policy(retry_policy)
        .build()
        .await?;
    let res = svc.generate_content().with_request(r.req).send().await?;
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
