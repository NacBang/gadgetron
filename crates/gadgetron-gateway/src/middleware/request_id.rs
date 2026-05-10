use axum::{extract::Request, middleware::Next, response::Response};
use uuid::Uuid;

/// Generates a `Uuid::new_v4()` per request, inserts it into request extensions,
/// and attaches it as the `x-request-id` response header.
///
/// §2.B.8 layer 3 (outermost auth-stack layer). Budget: ~100ns overhead.
///
/// Extension insertion allows downstream middleware (TenantContextLayer) to
/// read the same UUID rather than generating a second one.
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4();

    // Insert into extensions so TenantContextLayer can reuse this UUID.
    req.extensions_mut().insert(request_id);

    let mut response = next.run(req).await;

    response.headers_mut().insert(
        "x-request-id",
        request_id
            .to_string()
            .parse()
            .expect("UUID is always a valid header value"),
    );

    response
}
