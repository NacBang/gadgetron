use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use gadgetron_core::error::GadgetronError;

pub struct ApiError(pub GadgetronError);

impl From<GadgetronError> for ApiError {
    fn from(err: GadgetronError) -> Self {
        Self(err)
    }
}

/// Seconds until the next quota reset window, used to populate the
/// `Retry-After` header and `retry_after_seconds` field on 429
/// responses (ISSUE 11 TASK 11.1).
///
/// Today we assume a rolling 60-second window for rate limits — a
/// conservative upper bound that tells honest clients "wait a
/// minute before retrying" without requiring the handler to know
/// the actual refill time. When the ISSUE 11 TASK 11.2 token-bucket
/// enforcer lands, this will thread the real refill time through
/// the token so the header reports the exact countdown.
const QUOTA_RETRY_AFTER_SECONDS: u32 = 60;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let err = &self.0;
        let status = StatusCode::from_u16(err.http_status_code())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        // ISSUE 11 TASK 11.1 — quota-exceeded responses carry a
        // structured retry hint so SDK clients can back off
        // automatically instead of retrying in tight loops. 429 is
        // the only status that gets the hint today; other error
        // bodies keep the base shape.
        let retry_after = if status == StatusCode::TOO_MANY_REQUESTS {
            Some(QUOTA_RETRY_AFTER_SECONDS)
        } else {
            None
        };

        let body = if let Some(secs) = retry_after {
            serde_json::json!({
                "error": {
                    "message": err.error_message(),
                    "type": err.error_type(),
                    "code": err.error_code(),
                    "retry_after_seconds": secs,
                }
            })
        } else {
            serde_json::json!({
                "error": {
                    "message": err.error_message(),
                    "type": err.error_type(),
                    "code": err.error_code(),
                }
            })
        };

        tracing::error!(
            error.code = err.error_code(),
            error.type_ = err.error_type(),
            "{}",
            err
        );

        let mut headers = HeaderMap::new();
        if let Some(secs) = retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }

        (status, headers, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::error::{DatabaseErrorKind, NodeErrorKind};
    use uuid::Uuid;

    fn status_of(err: GadgetronError) -> u16 {
        err.http_status_code()
    }

    #[test]
    fn config_400() {
        assert_eq!(status_of(GadgetronError::Config("bad".into())), 400);
    }

    #[test]
    fn provider_502() {
        assert_eq!(status_of(GadgetronError::Provider("down".into())), 502);
    }

    #[test]
    fn tenant_not_found_401() {
        assert_eq!(status_of(GadgetronError::TenantNotFound), 401);
    }

    #[test]
    fn forbidden_403() {
        assert_eq!(status_of(GadgetronError::Forbidden), 403);
    }

    #[test]
    fn quota_exceeded_429() {
        assert_eq!(
            status_of(GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }),
            429
        );
    }

    #[test]
    fn routing_503() {
        assert_eq!(status_of(GadgetronError::Routing("none".into())), 503);
    }

    #[test]
    fn db_pool_timeout_503() {
        assert_eq!(
            status_of(GadgetronError::Database {
                kind: DatabaseErrorKind::PoolTimeout,
                message: "".into()
            }),
            503
        );
    }

    #[test]
    fn db_row_not_found_404() {
        assert_eq!(
            status_of(GadgetronError::Database {
                kind: DatabaseErrorKind::RowNotFound,
                message: "".into()
            }),
            404
        );
    }

    #[test]
    fn node_invalid_mig_400() {
        assert_eq!(
            status_of(GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into()
            }),
            400
        );
    }

    #[test]
    fn message_differs_from_code() {
        let err = GadgetronError::TenantNotFound;
        assert_ne!(err.error_message(), err.error_code());
        assert!(err.error_message().len() > 20);
    }

    #[test]
    fn openai_error_type_taxonomy() {
        assert_eq!(
            GadgetronError::TenantNotFound.error_type(),
            "authentication_error"
        );
        assert_eq!(GadgetronError::Forbidden.error_type(), "permission_error");
        assert_eq!(
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil()
            }
            .error_type(),
            "quota_error"
        );
    }

    #[test]
    fn all_12_variants_have_valid_status() {
        let variants: Vec<GadgetronError> = vec![
            GadgetronError::Config("".into()),
            GadgetronError::Provider("".into()),
            GadgetronError::Routing("".into()),
            GadgetronError::StreamInterrupted { reason: "".into() },
            GadgetronError::QuotaExceeded {
                tenant_id: Uuid::nil(),
            },
            GadgetronError::TenantNotFound,
            GadgetronError::Forbidden,
            GadgetronError::Billing("".into()),
            GadgetronError::DownloadFailed("".into()),
            GadgetronError::HotSwapFailed("".into()),
            GadgetronError::Database {
                kind: DatabaseErrorKind::Other,
                message: "".into(),
            },
            GadgetronError::Node {
                kind: NodeErrorKind::InvalidMigProfile,
                message: "".into(),
            },
        ];
        for v in &variants {
            let s = v.http_status_code();
            assert!((400..600).contains(&s), "{:?} has status {}", v, s);
        }
    }

    #[test]
    fn error_body_never_leaks_internal() {
        let err = GadgetronError::Database {
            kind: DatabaseErrorKind::QueryFailed,
            message: "SELECT * FROM secret_table".into(),
        };
        let msg = err.error_message();
        assert!(!msg.contains("secret_table"));
        assert!(!msg.contains("SELECT"));
    }

    #[test]
    fn api_error_from_gadgetron_error() {
        let ge = GadgetronError::Forbidden;
        let ae: ApiError = ge.into();
        assert_eq!(ae.0.http_status_code(), 403);
    }

    #[tokio::test]
    async fn quota_exceeded_response_carries_retry_after_header_and_field() {
        // ISSUE 11 TASK 11.1 — 429 responses must include both the
        // `Retry-After` header and a `retry_after_seconds` body
        // field so SDK clients can back off deterministically.
        use axum::body::to_bytes;
        use axum::response::IntoResponse;

        let ae = ApiError(GadgetronError::QuotaExceeded {
            tenant_id: Uuid::nil(),
        });
        let resp = ae.into_response();
        assert_eq!(resp.status(), 429);
        let retry_header = resp
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .expect("Retry-After header must be set on 429")
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(retry_header, "60");

        let body_bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"]["retry_after_seconds"], 60);
        assert_eq!(body["error"]["code"], "quota_exceeded");
    }

    #[tokio::test]
    async fn non_429_responses_omit_retry_after() {
        // Other 4xx / 5xx paths must NOT carry Retry-After — that
        // header has specific semantics clients act on, and
        // sending it with a 404 would confuse retry logic.
        use axum::body::to_bytes;
        use axum::response::IntoResponse;

        let ae = ApiError(GadgetronError::Forbidden);
        let resp = ae.into_response();
        assert_eq!(resp.status(), 403);
        assert!(
            resp.headers()
                .get(axum::http::header::RETRY_AFTER)
                .is_none(),
            "403 must not have Retry-After"
        );
        let body_bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(
            body["error"].get("retry_after_seconds").is_none(),
            "non-429 body must not have retry_after_seconds"
        );
    }
}
