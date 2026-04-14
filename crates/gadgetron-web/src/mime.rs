//! MIME type resolution with a fast-path table for the common Next.js static-export
//! extensions, falling back to `mime_guess` for the long tail.

use axum::http::HeaderValue;

/// Returns a `Content-Type` header value for the given path.
///
/// Fast path: match on the extension and return a `HeaderValue::from_static` for the
/// known Next.js output types. Allocation-free for 99% of requests.
///
/// Slow path: delegate to `mime_guess`, then validate the resulting string.
/// Falls back to `application/octet-stream` on any error.
pub fn content_type_for(path: &str) -> HeaderValue {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "html" => HeaderValue::from_static("text/html; charset=utf-8"),
        "js" => HeaderValue::from_static("application/javascript; charset=utf-8"),
        "mjs" => HeaderValue::from_static("application/javascript; charset=utf-8"),
        "css" => HeaderValue::from_static("text/css; charset=utf-8"),
        "json" => HeaderValue::from_static("application/json"),
        "woff2" => HeaderValue::from_static("font/woff2"),
        "woff" => HeaderValue::from_static("font/woff"),
        "svg" => HeaderValue::from_static("image/svg+xml"),
        "png" => HeaderValue::from_static("image/png"),
        "jpg" | "jpeg" => HeaderValue::from_static("image/jpeg"),
        "ico" => HeaderValue::from_static("image/x-icon"),
        "map" => HeaderValue::from_static("application/json"),
        "txt" => HeaderValue::from_static("text/plain; charset=utf-8"),
        _ => {
            let guess = mime_guess::from_path(path)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            HeaderValue::from_str(&guess)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_path_returns_static() {
        assert_eq!(
            content_type_for("index.html"),
            HeaderValue::from_static("text/html; charset=utf-8")
        );
        assert_eq!(
            content_type_for("_next/static/abc.js"),
            HeaderValue::from_static("application/javascript; charset=utf-8")
        );
        assert_eq!(
            content_type_for("favicon.ico"),
            HeaderValue::from_static("image/x-icon")
        );
    }

    #[test]
    fn unknown_extension_falls_through() {
        // mime_guess may or may not know `.quux`; the fallback covers both cases.
        let ct = content_type_for("nope.quux");
        // Should be a valid HeaderValue either way.
        assert!(!ct.is_empty());
    }
}
