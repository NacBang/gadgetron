//! Domain-neutral, versioned autonomy policy model.
//!
//! This module owns deterministic evaluation only. Runtime enforcement across
//! Penny, background jobs, Bundle Gadgets, Workbench, and Review resume is the
//! R3.2b boundary.

use std::collections::{BTreeSet, HashSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::agent::{
    tools::{GadgetDispatchContext, GadgetSchema, GadgetTier},
    GadgetMode, GadgetsConfig,
};

pub const POLICY_SCHEMA_VERSION: u32 = 1;
const MAX_RULES: usize = 256;
const MAX_REFERENCES: usize = 128;
const MAX_MATCH_VALUES: usize = 128;
const MAX_SCOPES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Auto,
    Review,
    Deny,
}

impl PolicyDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Review => "review",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Read,
    Write,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRisk {
    Unrated,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Missing,
    Sufficient,
    Stale,
    Contradictory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeState {
    Missing,
    Verifiable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackState {
    Unknown,
    Unavailable,
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceAssessment {
    pub state: EvidenceState,
    #[serde(default)]
    pub references: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeAssessment {
    pub state: OutcomeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackAssessment {
    pub state: RollbackState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensating_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyInput {
    pub action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gadget_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_hash: Option<String>,
    pub namespace: String,
    pub effect: PolicyEffect,
    pub risk: PolicyRisk,
    #[serde(default)]
    pub requested_scopes: BTreeSet<String>,
    #[serde(default)]
    pub actor_scopes: BTreeSet<String>,
    pub evidence: EvidenceAssessment,
    pub outcome: OutcomeAssessment,
    pub rollback: RollbackAssessment,
}

impl PolicyInput {
    pub fn for_gadget(
        context: &GadgetDispatchContext,
        name: &str,
        metadata: &GadgetPolicyMetadata,
    ) -> Result<Self, PolicyError> {
        let input = Self {
            action_id: name.to_string(),
            gadget_name: Some(name.to_string()),
            parameters_hash: None,
            namespace: name.split('.').next().unwrap_or(name).to_string(),
            effect: metadata.effect,
            risk: metadata.risk,
            requested_scopes: metadata.requested_scopes.clone(),
            actor_scopes: context.scopes.iter().cloned().collect(),
            evidence: EvidenceAssessment {
                state: if metadata.requires_evidence {
                    EvidenceState::Missing
                } else {
                    EvidenceState::Sufficient
                },
                references: BTreeSet::new(),
            },
            outcome: OutcomeAssessment {
                state: if metadata.outcome_verifiable {
                    OutcomeState::Verifiable
                } else {
                    OutcomeState::Missing
                },
                predicate_ref: metadata.outcome_ref.clone(),
            },
            rollback: RollbackAssessment {
                state: if metadata.rollback_available {
                    RollbackState::Available
                } else if metadata.effect == PolicyEffect::Destructive {
                    RollbackState::Unavailable
                } else {
                    RollbackState::Unknown
                },
                compensating_action: metadata.rollback_ref.clone(),
            },
        };
        input.validate()?;
        Ok(input)
    }

    pub fn with_parameters(mut self, parameters: &serde_json::Value) -> Result<Self, PolicyError> {
        self.parameters_hash = Some(digest(&canonical_json(parameters))?);
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), PolicyError> {
        validate_label("action_id", &self.action_id, 160)?;
        validate_label("namespace", &self.namespace, 64)?;
        if let Some(name) = &self.gadget_name {
            validate_label("gadget_name", name, 160)?;
        }
        if let Some(hash) = &self.parameters_hash {
            validate_digest("parameters_hash", hash)?;
        }
        if self.evidence.references.len() > MAX_REFERENCES {
            return Err(PolicyError::Invalid(format!(
                "evidence references exceed {MAX_REFERENCES}"
            )));
        }
        validate_set_size("requested scopes", self.requested_scopes.len(), MAX_SCOPES)?;
        validate_set_size("actor scopes", self.actor_scopes.len(), MAX_SCOPES)?;
        for value in self
            .requested_scopes
            .iter()
            .chain(self.actor_scopes.iter())
            .chain(self.evidence.references.iter())
        {
            validate_label("policy input reference", value, 256)?;
        }
        validate_optional_label(
            "outcome predicate",
            self.outcome.predicate_ref.as_deref(),
            256,
        )?;
        validate_optional_label(
            "compensating action",
            self.rollback.compensating_action.as_deref(),
            160,
        )?;
        Ok(())
    }

    pub fn digest(&self) -> Result<String, PolicyError> {
        self.validate()?;
        digest(self)
    }
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            let mut normalized = serde_json::Map::new();
            for key in keys {
                normalized.insert(key.clone(), canonical_json(&values[key]));
            }
            serde_json::Value::Object(normalized)
        }
        scalar => scalar.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetPolicyMetadata {
    pub effect: PolicyEffect,
    pub risk: PolicyRisk,
    pub requested_scopes: BTreeSet<String>,
    pub requires_evidence: bool,
    pub outcome_verifiable: bool,
    pub outcome_ref: Option<String>,
    pub rollback_available: bool,
    pub rollback_ref: Option<String>,
}

impl GadgetPolicyMetadata {
    pub fn from_schema(schema: &GadgetSchema) -> Self {
        match schema.tier {
            GadgetTier::Read => Self {
                effect: PolicyEffect::Read,
                risk: PolicyRisk::Low,
                requested_scopes: BTreeSet::new(),
                requires_evidence: false,
                outcome_verifiable: true,
                outcome_ref: None,
                rollback_available: false,
                rollback_ref: None,
            },
            GadgetTier::Write => Self {
                effect: PolicyEffect::Write,
                risk: PolicyRisk::Medium,
                requested_scopes: BTreeSet::new(),
                requires_evidence: true,
                outcome_verifiable: false,
                outcome_ref: None,
                rollback_available: false,
                rollback_ref: None,
            },
            GadgetTier::Destructive => Self {
                effect: PolicyEffect::Destructive,
                risk: PolicyRisk::Critical,
                requested_scopes: BTreeSet::new(),
                requires_evidence: true,
                outcome_verifiable: false,
                outcome_ref: None,
                rollback_available: false,
                rollback_ref: None,
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyMatcher {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub action_ids: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub namespaces: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub effects: BTreeSet<PolicyEffect>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub risks: BTreeSet<PolicyRisk>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub scopes: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub evidence_states: BTreeSet<EvidenceState>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub outcome_states: BTreeSet<OutcomeState>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub rollback_states: BTreeSet<RollbackState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub id: String,
    pub priority: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "match", default)]
    pub matcher: PolicyMatcher,
    pub decision: PolicyDecision,
    pub reason: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDocument {
    pub schema_version: u32,
    pub default_decision: PolicyDecision,
    pub default_reason: String,
    pub rules: Vec<PolicyRule>,
}

impl PolicyDocument {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if self.schema_version != POLICY_SCHEMA_VERSION {
            return Err(PolicyError::Invalid(format!(
                "unsupported policy schema version {}",
                self.schema_version
            )));
        }
        if self.rules.len() > MAX_RULES {
            return Err(PolicyError::Invalid(format!(
                "policy rules exceed {MAX_RULES}"
            )));
        }
        validate_label("default_reason", &self.default_reason, 240)?;
        let mut ids = HashSet::new();
        for rule in &self.rules {
            if !valid_rule_id(&rule.id) {
                return Err(PolicyError::Invalid(format!(
                    "rule id {:?} must be lowercase kebab-case (1-80 chars)",
                    rule.id
                )));
            }
            if !ids.insert(rule.id.as_str()) {
                return Err(PolicyError::Invalid(format!(
                    "duplicate policy rule id {:?}",
                    rule.id
                )));
            }
            validate_label("rule reason", &rule.reason, 240)?;
            validate_set_size(
                "rule action ids",
                rule.matcher.action_ids.len(),
                MAX_MATCH_VALUES,
            )?;
            validate_set_size(
                "rule namespaces",
                rule.matcher.namespaces.len(),
                MAX_MATCH_VALUES,
            )?;
            validate_set_size("rule scopes", rule.matcher.scopes.len(), MAX_SCOPES)?;
            for value in rule
                .matcher
                .action_ids
                .iter()
                .chain(rule.matcher.namespaces.iter())
                .chain(rule.matcher.scopes.iter())
            {
                validate_label("policy matcher", value, 160)?;
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, PolicyError> {
        self.validate()?;
        digest(self)
    }

    pub fn evaluate(
        &self,
        identity: PolicyIdentity,
        input: &PolicyInput,
    ) -> Result<PolicyDecisionTrace, PolicyError> {
        let document_hash = self.digest()?;
        if identity.document_hash != document_hash {
            return Err(PolicyError::IdentityHashMismatch);
        }
        let input_hash = input.digest()?;
        let missing_scopes: Vec<String> = input
            .requested_scopes
            .difference(&input.actor_scopes)
            .cloned()
            .collect();
        let mut steps = Vec::new();
        if !missing_scopes.is_empty() {
            let reason = format!(
                "Actor is missing required scope(s): {}",
                missing_scopes.join(", ")
            );
            steps.push(PolicyTraceStep {
                stage: PolicyTraceStage::ScopeGuard,
                rule_id: None,
                matched: false,
                failed_predicates: missing_scopes
                    .iter()
                    .map(|scope| format!("actor_scope:{scope}"))
                    .collect(),
                decision: Some(PolicyDecision::Deny),
                reason: reason.clone(),
            });
            return Ok(PolicyDecisionTrace {
                schema_version: POLICY_SCHEMA_VERSION,
                policy: identity,
                input_hash,
                decision: PolicyDecision::Deny,
                reason,
                steps,
            });
        }
        steps.push(PolicyTraceStep {
            stage: PolicyTraceStage::ScopeGuard,
            rule_id: None,
            matched: true,
            failed_predicates: Vec::new(),
            decision: None,
            reason: "Actor scopes satisfy the request".to_string(),
        });

        let mut rules: Vec<&PolicyRule> = self.rules.iter().filter(|rule| rule.enabled).collect();
        rules.sort_by(|left, right| (left.priority, &left.id).cmp(&(right.priority, &right.id)));
        for rule in rules {
            let failed = failed_predicates(&rule.matcher, input);
            let matched = failed.is_empty();
            steps.push(PolicyTraceStep {
                stage: PolicyTraceStage::Rule,
                rule_id: Some(rule.id.clone()),
                matched,
                failed_predicates: failed,
                decision: matched.then_some(rule.decision),
                reason: rule.reason.clone(),
            });
            if matched {
                return Ok(PolicyDecisionTrace {
                    schema_version: POLICY_SCHEMA_VERSION,
                    policy: identity,
                    input_hash,
                    decision: rule.decision,
                    reason: rule.reason.clone(),
                    steps,
                });
            }
        }

        steps.push(PolicyTraceStep {
            stage: PolicyTraceStage::Default,
            rule_id: None,
            matched: true,
            failed_predicates: Vec::new(),
            decision: Some(self.default_decision),
            reason: self.default_reason.clone(),
        });
        Ok(PolicyDecisionTrace {
            schema_version: POLICY_SCHEMA_VERSION,
            policy: identity,
            input_hash,
            decision: self.default_decision,
            reason: self.default_reason.clone(),
            steps,
        })
    }

    pub fn from_legacy_gadget_modes(modes: &GadgetsConfig) -> Result<Self, PolicyError> {
        modes
            .validate()
            .map_err(|error| PolicyError::Invalid(error.to_string()))?;
        let mut rules = vec![legacy_rule(
            "legacy-read",
            10,
            PolicyEffect::Read,
            None,
            GadgetMode::Auto,
        )];
        for (priority, (id, namespace, mode)) in [
            ("legacy-write-wiki", "wiki", modes.write.wiki_write),
            ("legacy-write-infra", "infra", modes.write.infra_write),
            (
                "legacy-write-scheduler",
                "scheduler",
                modes.write.scheduler_write,
            ),
            (
                "legacy-write-provider",
                "provider",
                modes.write.provider_mutate,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            rules.push(legacy_rule(
                id,
                20 + priority as u32,
                PolicyEffect::Write,
                Some(namespace),
                mode,
            ));
        }

        let mut namespaces = BTreeSet::new();
        namespaces.extend(modes.write.namespace_modes.keys().cloned());
        namespaces.extend(modes.write.legacy_namespace_modes.keys().map(|key| {
            key.strip_suffix("_admin")
                .or_else(|| key.strip_suffix("_write"))
                .unwrap_or(key)
                .to_string()
        }));
        for namespace in namespaces {
            if matches!(
                namespace.as_str(),
                "wiki" | "infra" | "scheduler" | "provider"
            ) {
                continue;
            }
            if let Some(mode) = modes.write.namespace_mode(&namespace) {
                rules.push(legacy_rule(
                    &format!("legacy-write-{}", sanitize_rule_id(&namespace)),
                    100 + rules.len() as u32,
                    PolicyEffect::Write,
                    Some(&namespace),
                    mode,
                ));
            }
        }
        rules.push(legacy_rule(
            "legacy-write-default",
            1_000,
            PolicyEffect::Write,
            None,
            modes.write.default_mode,
        ));
        rules.push(PolicyRule {
            id: "legacy-destructive".to_string(),
            priority: 2_000,
            enabled: true,
            matcher: PolicyMatcher {
                effects: BTreeSet::from([PolicyEffect::Destructive]),
                ..PolicyMatcher::default()
            },
            decision: if modes.destructive.enabled {
                PolicyDecision::Review
            } else {
                PolicyDecision::Deny
            },
            reason: if modes.destructive.enabled {
                "Legacy destructive tools require Review".to_string()
            } else {
                "Legacy destructive tools are disabled".to_string()
            },
        });
        let document = Self {
            schema_version: POLICY_SCHEMA_VERSION,
            default_decision: PolicyDecision::Review,
            default_reason: "No policy rule matched; Manager review is required".to_string(),
            rules,
        };
        document.validate()?;
        Ok(document)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyIdentity {
    pub policy_id: Uuid,
    pub revision: i64,
    pub document_hash: String,
}

impl PolicyIdentity {
    pub fn to_revision_ref(&self) -> String {
        format!(
            "{}:{}:{}",
            self.policy_id, self.revision, self.document_hash
        )
    }

    pub fn from_revision_ref(value: &str) -> Result<Self, PolicyError> {
        let mut parts = value.splitn(3, ':');
        let policy_id = parts
            .next()
            .and_then(|part| Uuid::parse_str(part).ok())
            .ok_or_else(|| {
                PolicyError::Invalid("policy revision reference has no valid id".into())
            })?;
        let revision = parts
            .next()
            .and_then(|part| part.parse::<i64>().ok())
            .filter(|revision| *revision > 0)
            .ok_or_else(|| {
                PolicyError::Invalid("policy revision reference has no valid revision".into())
            })?;
        let document_hash = parts
            .next()
            .filter(|hash| {
                hash.len() == 71
                    && hash.starts_with("sha256:")
                    && hash[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| {
                PolicyError::Invalid("policy revision reference has no valid hash".into())
            })?
            .to_string();
        Ok(Self {
            policy_id,
            revision,
            document_hash,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementPath {
    Tool,
    WorkbenchAction,
    ReviewResume,
    BundleBackground,
    KnowledgeBackground,
}

impl EnforcementPath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::WorkbenchAction => "workbench_action",
            Self::ReviewResume => "review_resume",
            Self::BundleBackground => "bundle_background",
            Self::KnowledgeBackground => "knowledge_background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyReviewState {
    None,
    Pending,
    Approved,
}

#[derive(Debug, Clone)]
pub struct PolicyEvaluationRequest {
    pub tenant_id: Uuid,
    pub path: EnforcementPath,
    pub input: PolicyInput,
    pub pinned_policy: Option<PolicyIdentity>,
    pub approval_id: Option<Uuid>,
    pub review_state: PolicyReviewState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAuthorization {
    Auto,
    Denied,
    PendingReview,
    ApprovedReview,
}

impl PolicyAuthorization {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Denied => "denied",
            Self::PendingReview => "pending_review",
            Self::ApprovedReview => "approved_review",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyEvaluation {
    pub event_id: Uuid,
    pub trace: PolicyDecisionTrace,
    pub trace_hash: String,
    pub authorization: PolicyAuthorization,
}

impl PolicyEvaluation {
    pub fn allows_execution(&self) -> bool {
        matches!(
            self.authorization,
            PolicyAuthorization::Auto | PolicyAuthorization::ApprovedReview
        )
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{detail}")]
pub struct PolicyEvaluationError {
    pub code: &'static str,
    pub detail: String,
}

#[async_trait]
pub trait PolicyEvaluator: Send + Sync + 'static {
    async fn active_identity(
        &self,
        tenant_id: Uuid,
    ) -> Result<PolicyIdentity, PolicyEvaluationError>;

    async fn evaluate(
        &self,
        request: PolicyEvaluationRequest,
    ) -> Result<PolicyEvaluation, PolicyEvaluationError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyTraceStage {
    ScopeGuard,
    Rule,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyTraceStep {
    pub stage: PolicyTraceStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    pub matched: bool,
    #[serde(default)]
    pub failed_predicates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<PolicyDecision>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecisionTrace {
    pub schema_version: u32,
    pub policy: PolicyIdentity,
    pub input_hash: String,
    pub decision: PolicyDecision,
    pub reason: String,
    pub steps: Vec<PolicyTraceStep>,
}

impl PolicyDecisionTrace {
    pub fn digest(&self) -> Result<String, PolicyError> {
        digest(self)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("invalid policy: {0}")]
    Invalid(String),
    #[error("policy identity hash does not match the document")]
    IdentityHashMismatch,
    #[error("policy serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

fn failed_predicates(matcher: &PolicyMatcher, input: &PolicyInput) -> Vec<String> {
    let mut failed = Vec::new();
    if !matcher.action_ids.is_empty() && !matcher.action_ids.contains(&input.action_id) {
        failed.push("action_id".to_string());
    }
    if !matcher.namespaces.is_empty() && !matcher.namespaces.contains(&input.namespace) {
        failed.push("namespace".to_string());
    }
    if !matcher.effects.is_empty() && !matcher.effects.contains(&input.effect) {
        failed.push("effect".to_string());
    }
    if !matcher.risks.is_empty() && !matcher.risks.contains(&input.risk) {
        failed.push("risk".to_string());
    }
    if !matcher.scopes.is_empty() && !matcher.scopes.is_subset(&input.requested_scopes) {
        failed.push("scope".to_string());
    }
    if !matcher.evidence_states.is_empty()
        && !matcher.evidence_states.contains(&input.evidence.state)
    {
        failed.push("evidence".to_string());
    }
    if !matcher.outcome_states.is_empty() && !matcher.outcome_states.contains(&input.outcome.state)
    {
        failed.push("outcome".to_string());
    }
    if !matcher.rollback_states.is_empty()
        && !matcher.rollback_states.contains(&input.rollback.state)
    {
        failed.push("rollback".to_string());
    }
    failed
}

fn legacy_rule(
    id: &str,
    priority: u32,
    effect: PolicyEffect,
    namespace: Option<&str>,
    mode: GadgetMode,
) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        priority,
        enabled: true,
        matcher: PolicyMatcher {
            namespaces: namespace.into_iter().map(ToString::to_string).collect(),
            effects: BTreeSet::from([effect]),
            ..PolicyMatcher::default()
        },
        decision: match mode {
            GadgetMode::Auto => PolicyDecision::Auto,
            GadgetMode::Ask => PolicyDecision::Review,
            GadgetMode::Never => PolicyDecision::Deny,
        },
        reason: format!(
            "Legacy {} mode maps to {}",
            match mode {
                GadgetMode::Auto => "Auto",
                GadgetMode::Ask => "Ask",
                GadgetMode::Never => "Never",
            },
            match mode {
                GadgetMode::Auto => "automatic execution",
                GadgetMode::Ask => "Manager review",
                GadgetMode::Never => "denial",
            }
        ),
    }
}

fn sanitize_rule_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn valid_rule_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_label(field: &str, value: &str, max: usize) -> Result<(), PolicyError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(PolicyError::Invalid(format!(
            "{field} must be non-empty single-line text up to {max} bytes"
        )));
    }
    Ok(())
}

fn validate_optional_label(
    field: &str,
    value: Option<&str>,
    max: usize,
) -> Result<(), PolicyError> {
    if let Some(value) = value {
        validate_label(field, value, max)?;
    }
    Ok(())
}

fn validate_set_size(field: &str, len: usize, max: usize) -> Result<(), PolicyError> {
    if len > max {
        return Err(PolicyError::Invalid(format!("{field} exceed {max}")));
    }
    Ok(())
}

fn validate_digest(field: &str, value: &str) -> Result<(), PolicyError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PolicyError::Invalid(format!(
            "{field} must be a lowercase sha256 digest"
        )));
    }
    Ok(())
}

fn digest<T: Serialize>(value: &T) -> Result<String, PolicyError> {
    let encoded = serde_json::to_vec(value)?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(encoded))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> PolicyInput {
        PolicyInput {
            action_id: "server.restart".to_string(),
            gadget_name: Some("server.restart".to_string()),
            parameters_hash: None,
            namespace: "server".to_string(),
            effect: PolicyEffect::Write,
            risk: PolicyRisk::Medium,
            requested_scopes: BTreeSet::from(["management".to_string()]),
            actor_scopes: BTreeSet::from(["management".to_string()]),
            evidence: EvidenceAssessment {
                state: EvidenceState::Sufficient,
                references: BTreeSet::from(["alert:42".to_string()]),
            },
            outcome: OutcomeAssessment {
                state: OutcomeState::Verifiable,
                predicate_ref: Some("service-ready-v1".to_string()),
            },
            rollback: RollbackAssessment {
                state: RollbackState::Available,
                compensating_action: Some("server.restart-previous".to_string()),
            },
        }
    }

    fn document() -> PolicyDocument {
        PolicyDocument {
            schema_version: POLICY_SCHEMA_VERSION,
            default_decision: PolicyDecision::Review,
            default_reason: "Review unmatched action".to_string(),
            rules: vec![PolicyRule {
                id: "bounded-write".to_string(),
                priority: 10,
                enabled: true,
                matcher: PolicyMatcher {
                    effects: BTreeSet::from([PolicyEffect::Write]),
                    risks: BTreeSet::from([PolicyRisk::Low, PolicyRisk::Medium]),
                    scopes: BTreeSet::from(["management".to_string()]),
                    evidence_states: BTreeSet::from([EvidenceState::Sufficient]),
                    outcome_states: BTreeSet::from([OutcomeState::Verifiable]),
                    rollback_states: BTreeSet::from([RollbackState::Available]),
                    ..PolicyMatcher::default()
                },
                decision: PolicyDecision::Auto,
                reason: "Bounded reversible write has complete evidence".to_string(),
            }],
        }
    }

    fn identity(document: &PolicyDocument) -> PolicyIdentity {
        PolicyIdentity {
            policy_id: Uuid::nil(),
            revision: 1,
            document_hash: document.digest().unwrap(),
        }
    }

    #[test]
    fn identical_policy_and_input_produce_identical_trace_and_hash() {
        let document = document();
        let first = document.evaluate(identity(&document), &input()).unwrap();
        let second = document.evaluate(identity(&document), &input()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.digest().unwrap(), second.digest().unwrap());
        assert_eq!(first.decision, PolicyDecision::Auto);
    }

    #[test]
    fn missing_actor_scope_denies_before_rules() {
        let document = document();
        let mut request = input();
        request.actor_scopes.clear();
        let trace = document.evaluate(identity(&document), &request).unwrap();
        assert_eq!(trace.decision, PolicyDecision::Deny);
        assert_eq!(trace.steps.len(), 1);
        assert_eq!(trace.steps[0].stage, PolicyTraceStage::ScopeGuard);
    }

    #[test]
    fn risk_evidence_outcome_and_rollback_are_each_visible_in_trace() {
        let document = document();
        let mut request = input();
        request.risk = PolicyRisk::High;
        request.evidence.state = EvidenceState::Contradictory;
        request.outcome.state = OutcomeState::Missing;
        request.rollback.state = RollbackState::Unavailable;
        let trace = document.evaluate(identity(&document), &request).unwrap();
        assert_eq!(trace.decision, PolicyDecision::Review);
        assert_eq!(
            trace.steps[1].failed_predicates,
            ["risk", "evidence", "outcome", "rollback"]
        );
        assert_eq!(trace.steps.last().unwrap().stage, PolicyTraceStage::Default);
    }

    #[test]
    fn legacy_modes_preserve_auto_ask_never_namespace_and_destructive() {
        let mut modes = GadgetsConfig::default();
        modes.write.wiki_write = GadgetMode::Auto;
        modes.write.infra_write = GadgetMode::Ask;
        modes
            .write
            .namespace_modes
            .insert("server".to_string(), GadgetMode::Never);
        modes.destructive.enabled = true;
        let document = PolicyDocument::from_legacy_gadget_modes(&modes).unwrap();
        let policy = identity(&document);
        let mut request = input();

        request.namespace = "wiki".to_string();
        assert_eq!(
            document
                .evaluate(policy.clone(), &request)
                .unwrap()
                .decision,
            PolicyDecision::Auto
        );
        request.namespace = "infra".to_string();
        assert_eq!(
            document
                .evaluate(policy.clone(), &request)
                .unwrap()
                .decision,
            PolicyDecision::Review
        );
        request.namespace = "server".to_string();
        assert_eq!(
            document
                .evaluate(policy.clone(), &request)
                .unwrap()
                .decision,
            PolicyDecision::Deny
        );
        request.effect = PolicyEffect::Destructive;
        assert_eq!(
            document.evaluate(policy, &request).unwrap().decision,
            PolicyDecision::Review
        );
    }

    #[test]
    fn malformed_or_duplicate_rules_are_rejected() {
        let mut document = document();
        document.rules.push(document.rules[0].clone());
        assert!(matches!(document.validate(), Err(PolicyError::Invalid(_))));
        document.rules.pop();
        document.rules[0].id = "Not Valid".to_string();
        assert!(matches!(document.validate(), Err(PolicyError::Invalid(_))));
    }

    #[test]
    fn oversized_input_and_matcher_sets_are_rejected() {
        let mut request = input();
        request.requested_scopes = (0..=MAX_SCOPES)
            .map(|index| format!("scope-{index}"))
            .collect();
        assert!(request.validate().is_err());

        let mut policy = document();
        policy.rules[0].matcher.action_ids = (0..=MAX_MATCH_VALUES)
            .map(|index| format!("action-{index}"))
            .collect();
        assert!(policy.validate().is_err());
    }

    #[test]
    fn identity_hash_mismatch_is_rejected() {
        let document = document();
        let identity = PolicyIdentity {
            policy_id: Uuid::nil(),
            revision: 1,
            document_hash: "sha256:wrong".to_string(),
        };
        assert!(matches!(
            document.evaluate(identity, &input()),
            Err(PolicyError::IdentityHashMismatch)
        ));
    }

    #[test]
    fn serde_is_stable_for_rule_match_field() {
        let encoded = serde_json::to_value(document()).unwrap();
        assert!(encoded["rules"][0].get("match").is_some());
        let round_trip: PolicyDocument = serde_json::from_value(encoded).unwrap();
        assert_eq!(round_trip, document());
    }

    #[test]
    fn gadget_schema_metadata_is_conservative_for_mutations() {
        let read = GadgetPolicyMetadata::from_schema(&GadgetSchema {
            name: "wiki.read".into(),
            tier: GadgetTier::Read,
            description: "Read a note".into(),
            input_schema: serde_json::json!({"type": "object"}),
            idempotent: Some(true),
        });
        assert_eq!(read.effect, PolicyEffect::Read);
        assert_eq!(read.risk, PolicyRisk::Low);
        assert!(!read.requires_evidence);
        assert!(read.outcome_verifiable);

        let destructive = GadgetPolicyMetadata::from_schema(&GadgetSchema {
            name: "wiki.delete".into(),
            tier: GadgetTier::Destructive,
            description: "Delete a note".into(),
            input_schema: serde_json::json!({"type": "object"}),
            idempotent: Some(false),
        });
        assert_eq!(destructive.effect, PolicyEffect::Destructive);
        assert_eq!(destructive.risk, PolicyRisk::Critical);
        assert!(destructive.requires_evidence);
        assert!(!destructive.outcome_verifiable);
        assert!(!destructive.rollback_available);
    }

    #[test]
    fn policy_revision_reference_round_trips_and_rejects_drifted_shapes() {
        let document = document();
        let identity = identity(&document);
        assert_eq!(
            PolicyIdentity::from_revision_ref(&identity.to_revision_ref()).unwrap(),
            identity
        );
        assert!(PolicyIdentity::from_revision_ref("knowledge-source-read-v1").is_err());
        assert!(PolicyIdentity::from_revision_ref(&format!(
            "{}:0:{}",
            Uuid::new_v4(),
            document.digest().unwrap()
        ))
        .is_err());
    }

    #[test]
    fn parameter_binding_is_key_order_stable_and_value_sensitive() {
        let first = input()
            .with_parameters(&serde_json::json!({"target": "edge-1", "force": false}))
            .unwrap();
        let reordered = input()
            .with_parameters(&serde_json::json!({"force": false, "target": "edge-1"}))
            .unwrap();
        let changed = input()
            .with_parameters(&serde_json::json!({"target": "edge-2", "force": false}))
            .unwrap();
        assert_eq!(first.parameters_hash, reordered.parameters_hash);
        assert_eq!(first.digest().unwrap(), reordered.digest().unwrap());
        assert_ne!(first.parameters_hash, changed.parameters_hash);
        assert_ne!(first.digest().unwrap(), changed.digest().unwrap());
    }
}
