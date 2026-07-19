use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BundleId, BundleSdkError, CapabilityId, Result};

const MAX_ID: usize = 256;
const MAX_TEXT: usize = 2_048;
const MAX_PASSAGE: usize = 32_768;
const MAX_JSON_BYTES: usize = 1_048_576;
const MAX_SPACES: usize = 128;
const MAX_CITATIONS: usize = 256;
const MAX_GAPS: usize = 128;
const MAX_SOURCES: u32 = 1_000;
const MAX_ITEMS: u32 = 10_000;
const MAX_BYTES: u64 = 1_073_741_824;
const MAX_TOKENS: u64 = 10_000_000;
const MAX_TIMEOUT_SECONDS: u32 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IntelligenceAuthority {
    pub tenant_id: String,
    pub actor_id: String,
    pub allowed_space_ids: Vec<String>,
}

impl IntelligenceAuthority {
    pub fn new(
        tenant_id: impl Into<String>,
        actor_id: impl Into<String>,
        allowed_space_ids: Vec<String>,
    ) -> Result<Self> {
        let authority = Self {
            tenant_id: tenant_id.into(),
            actor_id: actor_id.into(),
            allowed_space_ids,
        };
        authority.validate("intelligence_authority")?;
        Ok(authority)
    }

    pub fn validate(&self, field: &str) -> Result<()> {
        bounded_nonempty(&format!("{field}.tenant_id"), &self.tenant_id, MAX_ID)?;
        bounded_nonempty(&format!("{field}.actor_id"), &self.actor_id, MAX_ID)?;
        if self.allowed_space_ids.is_empty() || self.allowed_space_ids.len() > MAX_SPACES {
            return Err(BundleSdkError::protocol(
                format!("{field}.allowed_space_ids"),
                format!("must contain 1-{MAX_SPACES} authorized Space ids"),
            ));
        }
        let mut previous: Option<&str> = None;
        for (index, space_id) in self.allowed_space_ids.iter().enumerate() {
            bounded_nonempty(
                &format!("{field}.allowed_space_ids[{index}]"),
                space_id,
                MAX_ID,
            )?;
            if previous.is_some_and(|value| value >= space_id.as_str()) {
                return Err(BundleSdkError::protocol(
                    format!("{field}.allowed_space_ids"),
                    "must be strictly sorted and unique for canonical authorization binding",
                ));
            }
            previous = Some(space_id);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SubjectRevisionRef {
    pub owner_bundle: BundleId,
    pub kind: CapabilityId,
    pub stable_id: String,
    pub revision: String,
}

impl SubjectRevisionRef {
    pub fn new(
        owner_bundle: BundleId,
        kind: CapabilityId,
        stable_id: impl Into<String>,
        revision: impl Into<String>,
    ) -> Result<Self> {
        let subject = Self {
            owner_bundle,
            kind,
            stable_id: stable_id.into(),
            revision: revision.into(),
        };
        subject.validate("subject_revision")?;
        Ok(subject)
    }

    fn validate(&self, field: &str) -> Result<()> {
        bounded_nonempty(&format!("{field}.stable_id"), &self.stable_id, MAX_ID)?;
        bounded_nonempty(&format!("{field}.revision"), &self.revision, MAX_ID)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IntelligenceBudget {
    pub max_sources: u32,
    pub max_items: u32,
    pub max_bytes: u64,
    pub max_tokens: u64,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IntelligenceQueryDraft {
    pub query_id: String,
    pub subject: SubjectRevisionRef,
    pub question: String,
    pub max_freshness_seconds: u64,
    pub budget: IntelligenceBudget,
}

impl IntelligenceQueryDraft {
    pub fn new(
        query_id: impl Into<String>,
        subject: SubjectRevisionRef,
        question: impl Into<String>,
        max_freshness_seconds: u64,
        budget: IntelligenceBudget,
    ) -> Result<Self> {
        let draft = Self {
            query_id: query_id.into(),
            subject,
            question: question.into(),
            max_freshness_seconds,
            budget,
        };
        draft.validate()?;
        Ok(draft)
    }

    pub fn bind(self, authority: IntelligenceAuthority) -> Result<IntelligenceQuery> {
        let query = IntelligenceQuery {
            query_id: self.query_id,
            authority,
            subject: self.subject,
            question: self.question,
            max_freshness_seconds: self.max_freshness_seconds,
            budget: self.budget,
        };
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> Result<()> {
        bounded_nonempty("intelligence_query_draft.query_id", &self.query_id, MAX_ID)?;
        self.subject.validate("intelligence_query_draft.subject")?;
        bounded_nonempty(
            "intelligence_query_draft.question",
            &self.question,
            MAX_TEXT,
        )?;
        if self.max_freshness_seconds == 0 {
            return Err(BundleSdkError::protocol(
                "intelligence_query_draft.max_freshness_seconds",
                "must be non-zero",
            ));
        }
        self.budget.validate("intelligence_query_draft.budget")
    }
}

impl IntelligenceBudget {
    pub fn new(
        max_sources: u32,
        max_items: u32,
        max_bytes: u64,
        max_tokens: u64,
        timeout_seconds: u32,
    ) -> Result<Self> {
        let budget = Self {
            max_sources,
            max_items,
            max_bytes,
            max_tokens,
            timeout_seconds,
        };
        budget.validate("intelligence_budget")?;
        Ok(budget)
    }

    fn validate(&self, field: &str) -> Result<()> {
        for (name, value, maximum) in [
            (
                "max_sources",
                u64::from(self.max_sources),
                u64::from(MAX_SOURCES),
            ),
            ("max_items", u64::from(self.max_items), u64::from(MAX_ITEMS)),
            ("max_bytes", self.max_bytes, MAX_BYTES),
            ("max_tokens", self.max_tokens, MAX_TOKENS),
            (
                "timeout_seconds",
                u64::from(self.timeout_seconds),
                u64::from(MAX_TIMEOUT_SECONDS),
            ),
        ] {
            if value == 0 || value > maximum {
                return Err(BundleSdkError::protocol(
                    format!("{field}.{name}"),
                    format!("must contain a non-zero value no greater than {maximum}"),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IntelligenceQuery {
    pub query_id: String,
    pub authority: IntelligenceAuthority,
    pub subject: SubjectRevisionRef,
    pub question: String,
    pub max_freshness_seconds: u64,
    pub budget: IntelligenceBudget,
}

impl IntelligenceQuery {
    pub fn validate(&self) -> Result<()> {
        bounded_nonempty("intelligence_query.query_id", &self.query_id, MAX_ID)?;
        self.authority.validate("intelligence_query.authority")?;
        self.subject.validate("intelligence_query.subject")?;
        bounded_nonempty("intelligence_query.question", &self.question, MAX_TEXT)?;
        if self.max_freshness_seconds == 0 {
            return Err(BundleSdkError::protocol(
                "intelligence_query.max_freshness_seconds",
                "must be non-zero",
            ));
        }
        self.budget.validate("intelligence_query.budget")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextCoverage {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CitationRole {
    Supporting,
    Contradicting,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeCitation {
    pub citation_id: String,
    pub space_id: String,
    pub owner_bundle: BundleId,
    pub source_id: String,
    pub source_revision: String,
    pub passage: String,
    pub role: CitationRole,
    pub applicability: String,
    pub freshness_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
}

impl KnowledgeCitation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        citation_id: impl Into<String>,
        space_id: impl Into<String>,
        owner_bundle: BundleId,
        source_id: impl Into<String>,
        source_revision: impl Into<String>,
        passage: impl Into<String>,
        role: CitationRole,
        applicability: impl Into<String>,
        freshness_seconds: u64,
        content_sha256: Option<String>,
    ) -> Self {
        Self {
            citation_id: citation_id.into(),
            space_id: space_id.into(),
            owner_bundle,
            source_id: source_id.into(),
            source_revision: source_revision.into(),
            passage: passage.into(),
            role,
            applicability: applicability.into(),
            freshness_seconds,
            content_sha256,
        }
    }

    fn validate(&self, field: &str, allowed_spaces: &BTreeSet<&str>) -> Result<()> {
        bounded_nonempty(&format!("{field}.citation_id"), &self.citation_id, MAX_ID)?;
        bounded_nonempty(&format!("{field}.space_id"), &self.space_id, MAX_ID)?;
        if !allowed_spaces.contains(self.space_id.as_str()) {
            return Err(BundleSdkError::protocol(
                format!("{field}.space_id"),
                "citation Space is outside the query authorization binding",
            ));
        }
        bounded_nonempty(&format!("{field}.source_id"), &self.source_id, MAX_ID)?;
        bounded_nonempty(
            &format!("{field}.source_revision"),
            &self.source_revision,
            MAX_ID,
        )?;
        bounded_multiline(&format!("{field}.passage"), &self.passage, MAX_PASSAGE)?;
        bounded_nonempty(
            &format!("{field}.applicability"),
            &self.applicability,
            MAX_TEXT,
        )?;
        if let Some(digest) = &self.content_sha256 {
            validate_sha256(&format!("{field}.content_sha256"), digest)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeContextPack {
    pub query_id: String,
    pub authority: IntelligenceAuthority,
    pub subject: SubjectRevisionRef,
    pub context_revision: String,
    pub coverage: ContextCoverage,
    #[serde(default)]
    pub citations: Vec<KnowledgeCitation>,
    #[serde(default)]
    pub gaps: Vec<String>,
}

impl KnowledgeContextPack {
    pub fn new(
        query: &IntelligenceQuery,
        context_revision: impl Into<String>,
        coverage: ContextCoverage,
        citations: Vec<KnowledgeCitation>,
        gaps: Vec<String>,
    ) -> Result<Self> {
        let pack = Self {
            query_id: query.query_id.clone(),
            authority: query.authority.clone(),
            subject: query.subject.clone(),
            context_revision: context_revision.into(),
            coverage,
            citations,
            gaps,
        };
        pack.validate_for_query(query)?;
        Ok(pack)
    }

    pub fn validate_for_query(&self, query: &IntelligenceQuery) -> Result<()> {
        query.validate()?;
        bounded_nonempty("knowledge_context.query_id", &self.query_id, MAX_ID)?;
        self.authority.validate("knowledge_context.authority")?;
        self.subject.validate("knowledge_context.subject")?;
        bounded_nonempty(
            "knowledge_context.context_revision",
            &self.context_revision,
            MAX_ID,
        )?;
        if self.query_id != query.query_id
            || self.authority != query.authority
            || self.subject != query.subject
        {
            return Err(BundleSdkError::protocol(
                "knowledge_context.binding",
                "query id, authority and subject revision must exactly match the IntelligenceQuery",
            ));
        }
        if self.citations.len() > MAX_CITATIONS || self.gaps.len() > MAX_GAPS {
            return Err(BundleSdkError::protocol(
                "knowledge_context",
                format!("exceeds {MAX_CITATIONS} citations or {MAX_GAPS} gaps"),
            ));
        }
        let allowed_spaces: BTreeSet<_> = query
            .authority
            .allowed_space_ids
            .iter()
            .map(String::as_str)
            .collect();
        let mut citation_ids = BTreeSet::new();
        for (index, citation) in self.citations.iter().enumerate() {
            citation.validate(
                &format!("knowledge_context.citations[{index}]"),
                &allowed_spaces,
            )?;
            if !citation_ids.insert(citation.citation_id.as_str()) {
                return Err(BundleSdkError::protocol(
                    "knowledge_context.citations",
                    "citation ids must be unique",
                ));
            }
        }
        for (index, gap) in self.gaps.iter().enumerate() {
            bounded_nonempty(&format!("knowledge_context.gaps[{index}]"), gap, MAX_TEXT)?;
        }
        let serialized_bytes = serde_json::to_vec(self)
            .map_err(|error| BundleSdkError::protocol("knowledge_context", error.to_string()))?
            .len() as u64;
        let byte_limit = query.budget.max_bytes.min(MAX_JSON_BYTES as u64);
        if serialized_bytes > byte_limit {
            return Err(BundleSdkError::protocol(
                "knowledge_context",
                format!("exceeds the {byte_limit}-byte query response budget"),
            ));
        }
        match self.coverage {
            ContextCoverage::Complete if self.citations.is_empty() => {
                Err(BundleSdkError::protocol(
                    "knowledge_context.coverage",
                    "complete coverage requires at least one cited source",
                ))
            }
            ContextCoverage::Partial if self.citations.is_empty() || self.gaps.is_empty() => {
                Err(BundleSdkError::protocol(
                    "knowledge_context.coverage",
                    "partial coverage requires citations and explicit gaps",
                ))
            }
            ContextCoverage::Unavailable if !self.citations.is_empty() || self.gaps.is_empty() => {
                Err(BundleSdkError::protocol(
                    "knowledge_context.coverage",
                    "unavailable coverage requires no citations and at least one explicit gap",
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextUseRef {
    pub query_id: String,
    pub context_revision: String,
}

impl ContextUseRef {
    pub fn new(query_id: impl Into<String>, context_revision: impl Into<String>) -> Self {
        Self {
            query_id: query_id.into(),
            context_revision: context_revision.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CitationUseRef {
    pub citation_id: String,
    pub source_revision: String,
}

impl CitationUseRef {
    pub fn new(citation_id: impl Into<String>, source_revision: impl Into<String>) -> Self {
        Self {
            citation_id: citation_id.into(),
            source_revision: source_revision.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutcomePredicateResult {
    Satisfied,
    Failed,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OutcomeFeedback {
    pub feedback_id: String,
    pub authority: IntelligenceAuthority,
    pub subject: SubjectRevisionRef,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUseRef>,
    pub before_state: Value,
    pub after_state: Value,
    pub predicate_result: OutcomePredicateResult,
    pub verification_summary: String,
    #[serde(default)]
    pub used_citations: Vec<CitationUseRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OutcomeFeedbackDraft {
    pub feedback_id: String,
    pub subject: SubjectRevisionRef,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUseRef>,
    pub before_state: Value,
    pub after_state: Value,
    pub predicate_result: OutcomePredicateResult,
    pub verification_summary: String,
    #[serde(default)]
    pub used_citations: Vec<CitationUseRef>,
}

impl OutcomeFeedbackDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        feedback_id: impl Into<String>,
        subject: SubjectRevisionRef,
        operation_id: impl Into<String>,
        context: Option<ContextUseRef>,
        before_state: Value,
        after_state: Value,
        predicate_result: OutcomePredicateResult,
        verification_summary: impl Into<String>,
        used_citations: Vec<CitationUseRef>,
    ) -> Self {
        Self {
            feedback_id: feedback_id.into(),
            subject,
            operation_id: operation_id.into(),
            context,
            before_state,
            after_state,
            predicate_result,
            verification_summary: verification_summary.into(),
            used_citations,
        }
    }

    pub fn bind(self, authority: IntelligenceAuthority) -> Result<OutcomeFeedback> {
        let feedback = OutcomeFeedback {
            feedback_id: self.feedback_id,
            authority,
            subject: self.subject,
            operation_id: self.operation_id,
            context: self.context,
            before_state: self.before_state,
            after_state: self.after_state,
            predicate_result: self.predicate_result,
            verification_summary: self.verification_summary,
            used_citations: self.used_citations,
        };
        feedback.validate()?;
        Ok(feedback)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OutcomeFeedbackReceipt {
    pub feedback_id: String,
    pub experience_revision: String,
    pub duplicate: bool,
}

impl OutcomeFeedbackReceipt {
    pub fn new(
        feedback_id: impl Into<String>,
        experience_revision: impl Into<String>,
        duplicate: bool,
    ) -> Result<Self> {
        let receipt = Self {
            feedback_id: feedback_id.into(),
            experience_revision: experience_revision.into(),
            duplicate,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<()> {
        bounded_nonempty(
            "outcome_feedback_receipt.feedback_id",
            &self.feedback_id,
            MAX_ID,
        )?;
        bounded_nonempty(
            "outcome_feedback_receipt.experience_revision",
            &self.experience_revision,
            MAX_ID,
        )
    }
}

impl OutcomeFeedback {
    pub fn validate(&self) -> Result<()> {
        bounded_nonempty("outcome_feedback.feedback_id", &self.feedback_id, MAX_ID)?;
        self.authority.validate("outcome_feedback.authority")?;
        self.subject.validate("outcome_feedback.subject")?;
        bounded_nonempty("outcome_feedback.operation_id", &self.operation_id, MAX_ID)?;
        if let Some(context) = &self.context {
            bounded_nonempty(
                "outcome_feedback.context.query_id",
                &context.query_id,
                MAX_ID,
            )?;
            bounded_nonempty(
                "outcome_feedback.context.context_revision",
                &context.context_revision,
                MAX_ID,
            )?;
        }
        validate_json_object("outcome_feedback.before_state", &self.before_state)?;
        validate_json_object("outcome_feedback.after_state", &self.after_state)?;
        bounded_nonempty(
            "outcome_feedback.verification_summary",
            &self.verification_summary,
            MAX_TEXT,
        )?;
        if self.used_citations.len() > MAX_CITATIONS {
            return Err(BundleSdkError::protocol(
                "outcome_feedback.used_citations",
                format!("must contain at most {MAX_CITATIONS} citation revisions"),
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, citation) in self.used_citations.iter().enumerate() {
            bounded_nonempty(
                &format!("outcome_feedback.used_citations[{index}].citation_id"),
                &citation.citation_id,
                MAX_ID,
            )?;
            bounded_nonempty(
                &format!("outcome_feedback.used_citations[{index}].source_revision"),
                &citation.source_revision,
                MAX_ID,
            )?;
            if !ids.insert(citation.citation_id.as_str()) {
                return Err(BundleSdkError::protocol(
                    "outcome_feedback.used_citations",
                    "citation ids must be unique",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProjectArtifactRef {
    pub tenant_id: String,
    pub actor_id: String,
    pub space_id: String,
    pub project_id: String,
    pub owner_bundle: BundleId,
    pub artifact_id: String,
    pub revision: String,
    pub media_type: String,
    pub content_sha256: String,
}

impl ProjectArtifactRef {
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("tenant_id", &self.tenant_id),
            ("actor_id", &self.actor_id),
            ("space_id", &self.space_id),
            ("project_id", &self.project_id),
            ("artifact_id", &self.artifact_id),
            ("revision", &self.revision),
            ("media_type", &self.media_type),
        ] {
            bounded_nonempty(&format!("project_artifact.{field}"), value, MAX_ID)?;
        }
        validate_sha256("project_artifact.content_sha256", &self.content_sha256)
    }
}

fn bounded_nonempty(field: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(BundleSdkError::protocol(
            field,
            format!("must contain 1-{max} characters and no control characters"),
        ))
    } else {
        Ok(())
    }
}

fn bounded_multiline(field: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty()
        || value.len() > max
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        Err(BundleSdkError::protocol(
            field,
            format!("must contain 1-{max} characters and only text layout controls"),
        ))
    } else {
        Ok(())
    }
}

fn validate_sha256(field: &str, value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(BundleSdkError::protocol(
            field,
            "must be a 64-character lowercase hexadecimal SHA-256 digest",
        ))
    }
}

fn validate_json_object(field: &str, value: &Value) -> Result<()> {
    if !value.is_object() {
        return Err(BundleSdkError::protocol(field, "must be a JSON object"));
    }
    let size = serde_json::to_vec(value)
        .map_err(|error| BundleSdkError::protocol(field, error.to_string()))?
        .len();
    if size > MAX_JSON_BYTES {
        return Err(BundleSdkError::protocol(
            field,
            format!("exceeds {MAX_JSON_BYTES} serialized bytes"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority() -> IntelligenceAuthority {
        IntelligenceAuthority {
            tenant_id: "tenant-1".into(),
            actor_id: "manager-1".into(),
            allowed_space_ids: vec!["project-alpha".into(), "team-ops".into()],
        }
    }

    fn subject() -> SubjectRevisionRef {
        SubjectRevisionRef {
            owner_bundle: BundleId::new("server-administrator").unwrap(),
            kind: CapabilityId::new("server.incident").unwrap(),
            stable_id: "incident-42".into(),
            revision: "7".into(),
        }
    }

    fn query() -> IntelligenceQuery {
        IntelligenceQuery {
            query_id: "query-1".into(),
            authority: authority(),
            subject: subject(),
            question: "Which cited procedures apply to this incident?".into(),
            max_freshness_seconds: 86_400,
            budget: IntelligenceBudget {
                max_sources: 20,
                max_items: 100,
                max_bytes: 10_000_000,
                max_tokens: 50_000,
                timeout_seconds: 300,
            },
        }
    }

    fn citation() -> KnowledgeCitation {
        KnowledgeCitation {
            citation_id: "citation-1".into(),
            space_id: "team-ops".into(),
            owner_bundle: BundleId::new("server-operations-intelligence").unwrap(),
            source_id: "vendor-advisory-9".into(),
            source_revision: "3".into(),
            passage: "Restart only after the controller reaches a quiescent state.\nVerify health after restart.".into(),
            role: CitationRole::Supporting,
            applicability: "Controller generation 4 with firmware 2.x".into(),
            freshness_seconds: 3_600,
            content_sha256: Some("a".repeat(64)),
        }
    }

    #[test]
    fn bundle_drafts_cannot_choose_tenant_actor_or_visible_spaces() {
        let draft = IntelligenceQueryDraft::new(
            "query-draft-1",
            subject(),
            "Which cited procedure applies?",
            3_600,
            IntelligenceBudget::new(4, 20, 32_768, 4_000, 10).unwrap(),
        )
        .unwrap();
        let wire = serde_json::to_value(&draft).unwrap();
        assert!(wire.get("authority").is_none());

        let bound = draft.bind(authority()).unwrap();
        assert_eq!(bound.authority.actor_id, "manager-1");
        assert_eq!(
            bound.authority.allowed_space_ids,
            ["project-alpha", "team-ops"]
        );

        let feedback = OutcomeFeedbackDraft::new(
            "feedback-draft-1",
            subject(),
            "operation-1",
            Some(ContextUseRef::new("query-draft-1", "context-1")),
            serde_json::json!({}),
            serde_json::json!({"healthy": true}),
            OutcomePredicateResult::Satisfied,
            "Health was reread after the operation",
            vec![CitationUseRef::new("citation-1", "3")],
        );
        let wire = serde_json::to_value(&feedback).unwrap();
        assert!(wire.get("authority").is_none());
        assert_eq!(
            feedback.bind(authority()).unwrap().authority.actor_id,
            "manager-1"
        );
    }

    #[test]
    fn query_and_pack_require_exact_actor_space_and_subject_revision_binding() {
        let query = query();
        query.validate().unwrap();
        let pack = KnowledgeContextPack {
            query_id: query.query_id.clone(),
            authority: query.authority.clone(),
            subject: query.subject.clone(),
            context_revision: "context-4".into(),
            coverage: ContextCoverage::Complete,
            citations: vec![citation()],
            gaps: Vec::new(),
        };
        pack.validate_for_query(&query).unwrap();

        let mut hidden = pack.clone();
        hidden.citations[0].space_id = "tenant-secret".into();
        assert!(hidden
            .validate_for_query(&query)
            .unwrap_err()
            .to_string()
            .contains("outside the query authorization"));

        let mut other_actor = pack.clone();
        other_actor.authority.actor_id = "manager-2".into();
        assert!(other_actor.validate_for_query(&query).is_err());

        let mut stale_subject = pack.clone();
        stale_subject.subject.revision = "6".into();
        assert!(stale_subject.validate_for_query(&query).is_err());

        let mut tiny_budget = query.clone();
        tiny_budget.budget.max_bytes = 128;
        assert!(pack
            .validate_for_query(&tiny_budget)
            .unwrap_err()
            .to_string()
            .contains("response budget"));
    }

    #[test]
    fn query_rejects_empty_space_or_budget_and_unavailable_pack_cannot_cite() {
        let mut no_space = query();
        no_space.authority.allowed_space_ids.clear();
        assert!(no_space.validate().is_err());

        let mut no_budget = query();
        no_budget.budget.max_sources = 0;
        assert!(no_budget.validate().is_err());

        let query = query();
        let unavailable = KnowledgeContextPack {
            query_id: query.query_id.clone(),
            authority: query.authority.clone(),
            subject: query.subject.clone(),
            context_revision: "context-5".into(),
            coverage: ContextCoverage::Unavailable,
            citations: vec![citation()],
            gaps: vec!["No enabled Intelligence provider".into()],
        };
        assert!(unavailable.validate_for_query(&query).is_err());
    }

    #[test]
    fn outcome_and_project_artifact_pin_authority_revisions_and_exact_bytes() {
        let feedback = OutcomeFeedback {
            feedback_id: "feedback-1".into(),
            authority: authority(),
            subject: subject(),
            operation_id: "operation-9".into(),
            context: Some(ContextUseRef {
                query_id: "query-1".into(),
                context_revision: "context-4".into(),
            }),
            before_state: serde_json::json!({"revision": 7, "status": "failed"}),
            after_state: serde_json::json!({"revision": 8, "status": "healthy"}),
            predicate_result: OutcomePredicateResult::Satisfied,
            verification_summary: "Service health reread succeeded".into(),
            used_citations: vec![CitationUseRef {
                citation_id: "citation-1".into(),
                source_revision: "3".into(),
            }],
        };
        feedback.validate().unwrap();

        let artifact = ProjectArtifactRef {
            tenant_id: "tenant-1".into(),
            actor_id: "manager-1".into(),
            space_id: "project-alpha".into(),
            project_id: "alpha".into(),
            owner_bundle: BundleId::new("coding").unwrap(),
            artifact_id: "patch-17".into(),
            revision: "2".into(),
            media_type: "text/x-diff".into(),
            content_sha256: "b".repeat(64),
        };
        artifact.validate().unwrap();

        let mut unpinned = artifact;
        unpinned.revision.clear();
        assert!(unpinned.validate().is_err());
    }
}
