use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    OpenAiCompat,
    Management,
    XaasAdmin,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiCompat => "openai_compat",
            Self::Management => "management",
            Self::XaasAdmin => "xaas_admin",
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub daily_limit_cents: i64,
    pub daily_used_cents: i64,
    pub monthly_limit_cents: i64,
    pub monthly_used_cents: i64,
}

impl QuotaSnapshot {
    pub fn remaining_daily_cents(&self) -> i64 {
        self.daily_limit_cents - self.daily_used_cents
    }
}

#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: Uuid,
    pub api_key_id: Uuid,
    pub scopes: Vec<Scope>,
    pub quota_snapshot: Arc<QuotaSnapshot>,
    pub request_id: Uuid,
    pub started_at: std::time::Instant,
}

impl TenantContext {
    pub fn has_scope(&self, required: Scope) -> bool {
        self.scopes.contains(&required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(scopes: Vec<Scope>) -> TenantContext {
        TenantContext {
            tenant_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            scopes,
            quota_snapshot: Arc::new(QuotaSnapshot {
                daily_limit_cents: 10_000,
                daily_used_cents: 500,
                monthly_limit_cents: 100_000,
                monthly_used_cents: 5_000,
            }),
            request_id: Uuid::new_v4(),
            started_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn has_scope_returns_true_for_present() {
        let ctx = make_ctx(vec![Scope::OpenAiCompat, Scope::Management]);
        assert!(ctx.has_scope(Scope::OpenAiCompat));
        assert!(ctx.has_scope(Scope::Management));
    }

    #[test]
    fn has_scope_returns_false_for_absent() {
        let ctx = make_ctx(vec![Scope::OpenAiCompat]);
        assert!(!ctx.has_scope(Scope::XaasAdmin));
    }

    #[test]
    fn scope_display() {
        assert_eq!(Scope::OpenAiCompat.to_string(), "openai_compat");
        assert_eq!(Scope::Management.to_string(), "management");
        assert_eq!(Scope::XaasAdmin.to_string(), "xaas_admin");
    }

    #[test]
    fn quota_remaining() {
        let q = QuotaSnapshot {
            daily_limit_cents: 10_000,
            daily_used_cents: 3_000,
            monthly_limit_cents: 100_000,
            monthly_used_cents: 0,
        };
        assert_eq!(q.remaining_daily_cents(), 7_000);
    }

    #[test]
    fn tenant_context_clone_is_cheap() {
        let ctx = make_ctx(vec![Scope::OpenAiCompat]);
        let cloned = ctx.clone();
        assert_eq!(ctx.tenant_id, cloned.tenant_id);
        assert!(Arc::ptr_eq(&ctx.quota_snapshot, &cloned.quota_snapshot));
    }

    #[test]
    fn scope_is_non_exhaustive_and_serializable() {
        let scope = Scope::OpenAiCompat;
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, "\"OpenAiCompat\"");
        let de: Scope = serde_json::from_str(&json).unwrap();
        assert_eq!(de, scope);
    }
}
