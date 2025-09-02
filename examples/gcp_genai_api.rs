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

use crate::google::cloud::aiplatform::v1::prediction_service_client::PredictionServiceClient;
use crate::google::cloud::aiplatform::v1::{GenerateContentRequest, GenerateContentResponse};
use crate::worker_pool::{WorkerMetrics, WorkerPool};
use anyhow::anyhow;
use async_request_dispatcher::worker_pool;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use env_logger::Env;
use google_cloud_auth::credentials::{
    Builder, CacheableResource, Credentials, EntityTag, impersonated,
};
use http::StatusCode;
use http::{Extensions, HeaderMap};
use log::{debug, info};
use std::env::var;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::metadata::errors::InvalidMetadataValue;
use tonic::transport::{Channel, ClientTlsConfig};

tonic::include_proto!("gcp");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    let num_cpus = std::thread::available_parallelism()?;
    info!("number of CPUs available: {}", num_cpus);

    const AI_PLATFORM_ENDPOINT: &'static str = "https://aiplatform.googleapis.com";

    let credentials = ApiCredentials::build()?;
    let tls_config = ClientTlsConfig::new().with_native_roots();
    let channel = Channel::from_static(AI_PLATFORM_ENDPOINT)
        .tls_config(tls_config)?
        .connect()
        .await?;

    let workers =
        worker_pool::spawn_workers::<GenerateContentRequest, GenerateContentResponse, _, _, _>(
            num_cpus.into(),
            move |r| {
                let creds = credentials.clone();
                let channel = channel.clone();
                async move { process_request(r, channel, creds).await }
            },
        );

    let wp = WorkerPool::new(workers, Duration::from_secs(300));
    let app = Router::new()
        .route("/generate", post(generate_content))
        .route("/metrics", get(worker_metrics))
        .with_state(Api { wp });

    let addr = "0.0.0.0:8080";
    info!("request dispatcher listening: {}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
pub(crate) struct Api {
    pub(crate) wp: Arc<WorkerPool<GenerateContentRequest, GenerateContentResponse>>,
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

pub(crate) async fn generate_content(
    State(Api { wp }): State<Api>,
    headers: HeaderMap,
    req: Json<GenerateContentRequest>,
) -> ApiResult<Json<GenerateContentResponse>> {
    let session = headers.get("x-session-id").and_then(|v| v.to_str().ok());

    match wp.submit(session, req.0).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("error submitting job: {e}"),
        )),
    }
}

pub(crate) async fn worker_metrics(State(Api { wp }): State<Api>) -> Json<Vec<WorkerMetrics>> {
    Json(wp.worker_metrics())
}

pub(crate) async fn process_request(
    req: GenerateContentRequest,
    channel: Channel,
    mut creds: ApiCredentials,
) -> anyhow::Result<GenerateContentResponse> {
    info!("processing request: {:?}", &req);
    let authorization = creds.get_auth_header().await?;
    let auth = MetadataValue::try_from(authorization).map_err(ApiError::from)?;
    let mut client =
        PredictionServiceClient::with_interceptor(channel, move |mut req: Request<()>| {
            req.metadata_mut().insert("authorization", auth.clone());
            Ok(req)
        });
    let res = client.generate_content(req).await?.into_inner();
    debug!("api response: {:?}", &res);
    Ok(res)
}

#[derive(Clone, Debug)]
pub struct ApiCredentials {
    inner: Arc<Mutex<Credentials>>,
    tag: Option<EntityTag>,
    cached: HeaderMap,
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
        Ok(Self {
            inner: Arc::new(Mutex::new(credentials)),
            tag: None,
            cached: HeaderMap::new(),
        })
    }

    pub async fn get_headers(&mut self) -> anyhow::Result<HeaderMap> {
        let extensions = self.tag.iter().fold(Extensions::new(), |mut v, e| {
            v.insert(e.clone());
            v
        });

        let creds = self.inner.lock().await;
        match creds.headers(extensions).await? {
            CacheableResource::NotModified => Ok(self.cached.clone()),
            CacheableResource::New { entity_tag, data } => {
                self.cached = data;
                self.tag = Some(entity_tag);
                Ok(self.cached.clone())
            }
        }
    }

    pub async fn get_auth_header(&mut self) -> anyhow::Result<String> {
        let headers = self.get_headers().await?;
        Ok(headers
            .get("authorization")
            .ok_or_else(|| anyhow!("authorization error"))?
            .to_str()?
            .to_string())
    }
}

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("error occurred: {0}")]
    MetadataValueError(#[from] InvalidMetadataValue),
}

impl From<ApiError> for tonic::Status {
    fn from(e: ApiError) -> Self {
        Self::internal(format!("error occurred: {}", e))
    }
}
