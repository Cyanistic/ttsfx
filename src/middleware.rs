use std::time::Duration;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderValue;
use tracing::{debug, error, warn};

const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());

    request.extensions_mut().insert(RequestId(id.clone()));

    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[derive(Clone)]
pub struct RequestSpan;

impl<B> tower_http::trace::MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, request: &http::Request<B>) -> tracing::Span {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .map(|r| r.0.as_str())
            .unwrap_or("unknown");

        let client_ip = client_ip_from_headers(request);

        let method = request.method();
        let path = request.uri().path();

        tracing::info_span!(
            "request",
            id = %request_id,
            %method,
            %path,
            ip = %client_ip,
            otel.name = %format!("{} {}", method, path)
        )
    }
}

fn client_ip_from_headers<B>(request: &http::Request<B>) -> String {
    const FORWARDED: &str = "x-forwarded-for";
    const REAL_IP: &str = "x-real-ip";

    request
        .headers()
        .get(FORWARDED)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            request
                .headers()
                .get(REAL_IP)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Clone)]
pub struct StatusLevelOnResponse;

impl<B> tower_http::trace::OnResponse<B> for StatusLevelOnResponse {
    fn on_response(self, response: &http::Response<B>, latency: Duration, _span: &tracing::Span) {
        let status = response.status().as_u16();
        let latency_ms = latency.as_millis();

        match status {
            500.. => error!(status, latency_ms, "response"),
            400..500 => warn!(status, latency_ms, "response"),
            _ => debug!(status, latency_ms, "response"),
        }
    }
}
