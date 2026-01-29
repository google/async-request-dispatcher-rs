# GCP GenAI API Request Dispatcher Example

This example demonstrates how to build an asynchronous request dispatcher for GCP Vertex AI API using the `async-request-dispatcher` library.

## Overview

This example demonstrates an asynchronous request dispatcher that distributes traffic to multiple supported GCP regions for a given model.

## How It Works

The dispatcher acts as a proxy for the Vertex AI API:

1.  **Region Selection:** For each incoming request, the dispatcher selects a target region from the configured list using a Round-Robin strategy.
2.  **Request Routing:** The request is updated to target the selected region and then submitted to the internal worker pool.
3.  **API Execution:** A worker processes the request by calling the GCP API using the appropriate regional endpoint. The client is configured with an exponential backoff policy and automatically retries transient errors up to 3 times.

## Configuration

The application is configured via environment variables:

*   `GOOGLE_CLOUD_PROJECT`: Your Google Cloud Project ID.
*   `MODEL_ID`: The ID of the model to use (e.g., `gemini-1.5-flash-001`).
*   `MODEL_CONFIG`: A JSON string defining supported regions (e.g., `[{"model_id": "...", "supported_regions": ["us-central1", ...]}]`).

## Running the Example

1.  **Set Environment Variables:**

    ```bash
    export GOOGLE_CLOUD_PROJECT="your-project-id"
    export MODEL_ID="gemini-1.5-flash-001"
    export MODEL_CONFIG='[{"model_id": "gemini-1.5-flash-001", "supported_regions": ["us-central1", "us-east4"]}]'
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

    **Note:** The `MODEL_CONFIG` environment variable uses a complex JSON string. When passing it via `gcloud` command line, ensuring proper escaping is critical. The example below sets a configuration for `gemini-2.5-flash-lite` across many regions.

    ```bash
    gcloud run deploy gcp-genai-api \
      --image=<IMAGE> \
      --min-instances=0 \
      --set-env-vars=GOOGLE_CLOUD_PROJECT=<PROJECT> \
      --set-env-vars='^#^MODEL_CONFIG=[{"model_id":"gemini-2.5-flash-lite","supported_regions":["us-central1","us-east1","us-east4","us-east5","us-south1","us-west1","us-west4","europe-central2","europe-north1","europe-southwest1","europe-west1","europe-west4","europe-west8","europe-west9"]}]' \
      --set-env-vars=GOOGLE_GENAI_USE_VERTEXAI=True \
      --set-env-vars=MODEL_ID=gemini-2.5-flash-lite \
      --region=us-east4 \
      --project=<PROJECT>
    ```

    *   `^#^`: This prefix tells `gcloud` to use `#` as a delimiter instead of the default comma, which is necessary because the JSON string contains commas.