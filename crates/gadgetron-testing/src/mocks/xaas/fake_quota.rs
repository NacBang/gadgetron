use async_trait::async_trait;
use gadgetron_core::{context::QuotaSnapshot, error::GadgetronError};
use gadgetron_xaas::quota::enforcer::{QuotaEnforcer, QuotaToken};
use uuid::Uuid;

/// QuotaEnforcer that always rejects with `QuotaExceeded`.
///
/// Used in scenario 5 to assert that the gateway returns HTTP 429
/// when the quota enforcer denies the request.
pub struct ExhaustedQuotaEnforcer;

#[async_trait]
impl QuotaEnforcer for ExhaustedQuotaEnforcer {
    async fn check_pre(
        &self,
        tenant_id: Uuid,
        _snapshot: &QuotaSnapshot,
    ) -> Result<QuotaToken, GadgetronError> {
        Err(GadgetronError::QuotaExceeded { tenant_id })
    }

    async fn record_post(&self, _token: &QuotaToken, _actual_cost_cents: i64) {
        // No-op: quota was denied so post-record is never reached in normal flow.
    }
}
