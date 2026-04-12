use axum::response::IntoResponse;

pub async fn chat_completions() -> impl IntoResponse {
    // TODO: implement with Router + streaming SSE
    axum::Json(serde_json::json!({"error": "not implemented"}))
}

pub async fn list_models() -> impl IntoResponse {
    axum::Json(serde_json::json!({"data": []}))
}

pub async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({"status": "ok"}))
}

pub async fn list_nodes() -> impl IntoResponse {
    axum::Json(serde_json::json!({"nodes": []}))
}

pub async fn node_metrics() -> impl IntoResponse {
    axum::Json(serde_json::json!({"error": "not implemented"}))
}

pub async fn deploy_model() -> impl IntoResponse {
    axum::Json(serde_json::json!({"error": "not implemented"}))
}

pub async fn undeploy_model() -> impl IntoResponse {
    axum::Json(serde_json::json!({"error": "not implemented"}))
}

pub async fn model_status() -> impl IntoResponse {
    axum::Json(serde_json::json!({"models": []}))
}

pub async fn usage() -> impl IntoResponse {
    axum::Json(serde_json::json!({"usage": {}}))
}

pub async fn costs() -> impl IntoResponse {
    axum::Json(serde_json::json!({"costs": {}}))
}
