use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Info => "info",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
}

/// Result of `rules::classify` for one log line. `category` is a short
/// machine-readable token (`gpu_xid`, `oom_kill`, `nvme_failure`,
/// etc.) used to fold repeats; `summary` is the human-readable label.
/// `cause` / `solution` are operator-facing prose. `remediation` is
/// the optional click-to-run action (UI checks against a whitelist
/// before exposing the button).
#[derive(Debug, Clone)]
pub struct Classification {
    pub severity: Severity,
    pub category: String,
    /// Stable roll-up key for duplicate suppression. Same open
    /// `(tenant, host, source, fingerprint)` stays one finding row.
    pub fingerprint: String,
    pub summary: String,
    pub cause: Option<String>,
    pub solution: Option<String>,
    pub remediation: Option<serde_json::Value>,
}

/// One row in `log_findings`. Kept in sync with the migration column
/// list. `excerpt` is the originating log line (truncated to 1024 ch).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Finding {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub host_id: Uuid,
    pub source: String,
    pub severity: String,
    pub category: String,
    pub fingerprint: String,
    pub summary: String,
    pub excerpt: String,
    pub ts_first: DateTime<Utc>,
    pub ts_last: DateTime<Utc>,
    pub count: i32,
    pub dismissed_at: Option<DateTime<Utc>>,
    pub dismissed_by: Option<Uuid>,
    pub classified_by: String,
    pub cause: Option<String>,
    pub solution: Option<String>,
    pub remediation: Option<serde_json::Value>,
}
