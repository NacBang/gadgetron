use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use gadgetron_core::error::GadgetronError;

pub struct ApiError(pub GadgetronError);

impl From<GadgetronError> for ApiError {
    fn from(err: GadgetronError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let err = &self.0;
        let status = StatusCode::from_u16(err.http_status_code())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let body = serde_json::json!({
            "error": {
                "message": err.error_message(),
                "type": err.error_type(),
                "code": err.error_code(),
            }
        });

        tracing::error!(
            error.code = err.error_code(),
            error.type_ = err.error_type(),
            "{}",
            err
        );

        (status, axum::Json(body)).into_response()
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
}
