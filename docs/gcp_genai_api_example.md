# GCP GenAI API Request Dispatcher Example

This example demonstrates how to build an asynchronous request dispatcher for GCP Vertex AI API using the `async-request-dispatcher` library.

## Overview

This example demonstrates an asynchronous request dispatcher that adds queueing, backoff and retries support for a given model.

## How It Works

The dispatcher acts as a proxy for the Vertex AI API:

1.  **Request Routing:** The request is updated to target the global endpoint and then submitted to the internal worker pool.
3.  **API Execution:** A worker processes the request by calling the GCP API endpoint. The client is configured with an exponential backoff policy and automatically retries transient errors up to 3 times.
4.  **Persistence & Consistency:** The dispatcher uses a hybrid storage strategy. Active jobs are tracked in a high-performance in-memory `DashMap`. For multi-instance deployments, statuses are synchronized to **Google Cloud Firestore**, ensuring that any instance in the cluster can respond to status queries for any job.

## Configuration

The application is configured via environment variables:

*   `GOOGLE_CLOUD_PROJECT`: Your Google Cloud Project ID.
*   `FIRESTORE_COLLECTION`: (Optional) The Firestore collection name. Setting this enables distributed mode. If unset, the dispatcher runs in single-instance mode (in-memory only). Defaults to the example name (`gcp-genai-api`).
*   `FIRESTORE_DATABASE`: (Required if Firestore is enabled) The Firestore database ID.
*   `MODEL_ID`: The ID of the model to use (e.g., `gemini-1.5-flash-001`).

## Running the Example

1.  **Set Environment Variables:**

    ```bash
    export GOOGLE_CLOUD_PROJECT="your-project-id"
    export FIRESTORE_COLLECTION="example"
    export FIRESTORE_DATABASE="your-database-id"
    export MODEL_ID="gemini-1.5-flash-001"
    ```

2.  **Run the Server:**

    ```bash
    cargo run --example gcp-genai-api
    ```

3.  **Send a Request:**

    ```bash
    curl -X POST http://localhost:8080/generate \
      -H "Content-Type: application/json" \
      -d '{
        "contents": [{
          "parts": [{"text": "Explain quantum computing in simple terms."}]
        }]
      }'
    ```

## Deploy to Cloud Run

The included `Dockerfile` is configured to build and package this example.

1.  **Build and Push the Container Image:**

    ```bash
    gcloud builds submit --tag gcr.io/<PROJECT>/gcp-genai-api .
    ```

2.  **Deploy:**

    Use the following command to deploy the service. Adjust the `<IMAGE>` and `<PROJECT>` placeholders.

    ```bash
    gcloud run deploy gcp-genai-api \
      --image=<IMAGE> \
      --min-instances=0 \
      --set-env-vars=GOOGLE_CLOUD_PROJECT=<PROJECT> \
      --set-env-vars=FIRESTORE_COLLECTION=<COLLECTION> \
      --set-env-vars=FIRESTORE_DATABASE=<DATABASE> \
      --set-env-vars=GOOGLE_GENAI_USE_VERTEXAI=True \
      --set-env-vars=MODEL_ID=gemini-2.5-flash-lite \
      --region=us-east4 \
      --project=<PROJECT>
    ```

## Disclaimer

This is an example implementation and is not an officially supported Google product. It is provided "as is" without any warranties or guarantees.