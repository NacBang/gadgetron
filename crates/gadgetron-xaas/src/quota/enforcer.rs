use async_trait::async_trait;
use gadgetron_core::context::QuotaSnapshot;
use gadgetron_core::error::GadgetronError;
use uuid::Uuid;

#[derive(Debug)]
pub struct QuotaToken {
    pub tenant_id: Uuid,
    pub estimated_cost_cents: i64,
    used: std::sync::atomic::AtomicBool,
}

impl QuotaToken {
    pub fn new(tenant_id: Uuid, estimated_cost_cents: i64) -> Self {
        Self {
            tenant_id,
            estimated_cost_cents,
            used: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn mark_used(&self) {
        self.used.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn was_used(&self) -> bool {
        self.used.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
pub trait QuotaEnforcer: Send + Sync {
    async fn check_pre(
        &self,
        tenant_id: Uuid,
        snapshot: &QuotaSnapshot,
    ) -> Result<QuotaToken, GadgetronError>;

    async fn record_post(&self, token: &QuotaToken, actual_cost_cents: i64);
}

pub struct InMemoryQuotaEnforcer;

#[async_trait]
impl QuotaEnforcer for InMemoryQuotaEnforcer {
    async fn check_pre(
        &self,
        tenant_id: Uuid,
        snapshot: &QuotaSnapshot,
    ) -> Result<QuotaToken, GadgetronError> {
        if snapshot.remaining_daily_cents() <= 0 {
            return Err(GadgetronError::QuotaExceeded { tenant_id });
        }
        Ok(QuotaToken::new(tenant_id, 0))
    }

    async fn record_post(&self, token: &QuotaToken, _actual_cost_cents: i64) {
        token.mark_used();
    }
}

/// Composite enforcer that runs a per-tenant rate-limit check
/// BEFORE delegating the cost-based snapshot check to an inner
/// enforcer (ISSUE 11 TASK 11.2).
///
/// Rate-limit rejections surface as `GadgetronError::QuotaExceeded`
/// today — the gateway's `ApiError::into_response` (TASK 11.1)
/// already adds the `Retry-After` header + `retry_after_seconds`
/// body field to every 429, so clients get a usable back-off hint
/// without threading the limiter's exact refill time through the
/// error type. A future TASK can widen `GadgetronError` with a
/// dedicated variant carrying `retry_after_seconds` from the
/// limiter's `RateLimitedError`.
pub struct RateLimitedQuotaEnforcer {
    inner: std::sync::Arc<dyn QuotaEnforcer>,
    limiter: std::sync::Arc<crate::quota::rate_limit::TokenBucketRateLimiter>,
}

impl RateLimitedQuotaEnforcer {
    pub fn new(
        inner: std::sync::Arc<dyn QuotaEnforcer>,
        limiter: std::sync::Arc<crate::quota::rate_limit::TokenBucketRateLimiter>,
    ) -> Self {
        Self { inner, limiter }
    }
}

#[async_trait]
impl QuotaEnforcer for RateLimitedQuotaEnforcer {
    async fn check_pre(
        &self,
        tenant_id: Uuid,
        snapshot: &QuotaSnapshot,
    ) -> Result<QuotaToken, GadgetronError> {
        if let Err(rl) = self.limiter.consume(tenant_id) {
            tracing::info!(
                target: "quota.rate_limit",
                tenant_id = %tenant_id,
                retry_after_seconds = rl.retry_after_seconds,
                "tenant exceeded per-minute rate limit"
            );
            return Err(GadgetronError::QuotaExceeded { tenant_id });
        }
        self.inner.check_pre(tenant_id, snapshot).await
    }

    async fn record_post(&self, token: &QuotaToken, actual_cost_cents: i64) {
        self.inner.record_post(token, actual_cost_cents).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(daily_remaining: i64) -> QuotaSnapshot {
        QuotaSnapshot {
            daily_limit_cents: 10_000,
            daily_used_cents: 10_000 - daily_remaining,
            monthly_limit_cents: 100_000,
            monthly_used_cents: 0,
        }
    }

    #[tokio::test]
    async fn check_pre_allows_when_quota_remaining() {
        let enforcer = InMemoryQuotaEnforcer;
        let tid = Uuid::new_v4();
        let snapshot = make_snapshot(5_000);
        let token = enforcer.check_pre(tid, &snapshot).await.unwrap();
        assert_eq!(token.tenant_id, tid);
        assert!(!token.was_used());
    }

    #[tokio::test]
    async fn check_pre_rejects_when_quota_exhausted() {
        let enforcer = InMemoryQuotaEnforcer;
        let tid = Uuid::new_v4();
        let snapshot = make_snapshot(0);
        let err = enforcer.check_pre(tid, &snapshot).await.unwrap_err();
        match err {
            GadgetronError::QuotaExceeded { tenant_id } => assert_eq!(tenant_id, tid),
            _ => panic!("expected QuotaExceeded"),
        }
    }

    #[tokio::test]
    async fn check_pre_rejects_negative_remaining() {
        let enforcer = InMemoryQuotaEnforcer;
        let tid = Uuid::new_v4();
        let snapshot = make_snapshot(-100);
        assert!(enforcer.check_pre(tid, &snapshot).await.is_err());
    }

    #[tokio::test]
    async fn record_post_marks_token_used() {
        let enforcer = InMemoryQuotaEnforcer;
        let tid = Uuid::new_v4();
        let snapshot = make_snapshot(5_000);
        let token = enforcer.check_pre(tid, &snapshot).await.unwrap();
        assert!(!token.was_used());
        enforcer.record_post(&token, 100).await;
        assert!(token.was_used());
    }

    #[tokio::test]
    async fn quota_token_tracks_estimated_cost() {
        let token = QuotaToken::new(Uuid::new_v4(), 250);
        assert_eq!(token.estimated_cost_cents, 250);
    }
}
