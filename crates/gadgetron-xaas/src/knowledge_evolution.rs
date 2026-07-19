use std::collections::BTreeSet;

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

const MAX_CLAIMS: usize = 64;
const MAX_SCOPE_ITEMS: usize = 32;
const MAX_TEXT_BYTES: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeEvolutionTargetKind {
    Lesson,
    Insight,
}

impl KnowledgeEvolutionTargetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lesson => "lesson",
            Self::Insight => "insight",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeFreshnessStatus {
    Current,
    TimeSensitive,
    Unknown,
}

impl KnowledgeFreshnessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::TimeSensitive => "time_sensitive",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeImportanceFactorKind {
    OperationalImpact,
    EvidenceQuality,
    Novelty,
    Recurrence,
    CrossBundleReuse,
    ContradictionValue,
    OutcomeSupport,
}

impl KnowledgeImportanceFactorKind {
    const ALL: [Self; 7] = [
        Self::OperationalImpact,
        Self::EvidenceQuality,
        Self::Novelty,
        Self::Recurrence,
        Self::CrossBundleReuse,
        Self::ContradictionValue,
        Self::OutcomeSupport,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEvolutionClaim {
    pub id: String,
    pub statement: String,
    pub source_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFreshness {
    pub status: KnowledgeFreshnessStatus,
    #[serde(default)]
    pub review_after: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeImportanceFactor {
    pub factor: KnowledgeImportanceFactorKind,
    pub score: f32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEvolutionCandidate {
    pub schema_version: u32,
    #[serde(default)]
    pub dossier_artifact_id: Option<Uuid>,
    pub target_kind: KnowledgeEvolutionTargetKind,
    pub claim: String,
    pub claims: Vec<KnowledgeEvolutionClaim>,
    pub supporting_claim_ids: Vec<String>,
    #[serde(default)]
    pub contradicting_claim_ids: Vec<String>,
    pub applicability: Vec<String>,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub freshness: KnowledgeFreshness,
    pub confidence: f32,
    pub importance: Vec<KnowledgeImportanceFactor>,
    #[serde(default)]
    pub verified_outcome_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeEvolutionReadiness {
    ReadyForReview,
    NeedsOutcomeEvidence,
}

impl KnowledgeEvolutionCandidate {
    pub fn parse_and_validate(
        payload: Value,
        citations: &Value,
    ) -> Result<Self, KnowledgeEvolutionError> {
        let candidate: Self = serde_json::from_value(payload)
            .map_err(|error| KnowledgeEvolutionError::Invalid(error.to_string()))?;
        candidate.validate(citations)?;
        Ok(candidate)
    }

    pub fn validate(&self, citations: &Value) -> Result<(), KnowledgeEvolutionError> {
        if self.schema_version != 1 {
            return invalid("candidate schema_version must be 1");
        }
        validate_text("candidate claim", &self.claim, MAX_TEXT_BYTES)?;
        if self.claims.is_empty() || self.claims.len() > MAX_CLAIMS {
            return invalid("candidate must contain 1-64 structured claims");
        }
        validate_string_list("applicability", &self.applicability, true)?;
        validate_string_list("limitations", &self.limitations, false)?;
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return invalid("candidate confidence must be between zero and one");
        }

        let cited_sources = citation_source_ids(citations)?;
        let mut claim_ids = BTreeSet::new();
        for claim in &self.claims {
            validate_id("claim id", &claim.id)?;
            validate_text("claim statement", &claim.statement, MAX_TEXT_BYTES)?;
            if !claim_ids.insert(claim.id.as_str()) {
                return invalid("candidate contains a duplicate claim id");
            }
            if claim.source_ids.is_empty() {
                return invalid("every structured claim needs at least one Source");
            }
            let mut unique_sources = BTreeSet::new();
            for source_id in &claim.source_ids {
                if !unique_sources.insert(*source_id) {
                    return invalid("structured claim contains a duplicate Source");
                }
                if !cited_sources.contains(source_id) {
                    return invalid("structured claim references a Source outside its citations");
                }
            }
        }

        let supporting = validate_claim_refs(
            "supporting_claim_ids",
            &self.supporting_claim_ids,
            &claim_ids,
            true,
        )?;
        let contradicting = validate_claim_refs(
            "contradicting_claim_ids",
            &self.contradicting_claim_ids,
            &claim_ids,
            false,
        )?;
        if supporting.iter().any(|id| contradicting.contains(id)) {
            return invalid("a claim cannot both support and contradict the candidate");
        }
        if self.target_kind == KnowledgeEvolutionTargetKind::Insight {
            if supporting.len() < 2 {
                return invalid("an Insight candidate needs at least two supporting claims");
            }
            if self.limitations.is_empty() {
                return invalid("an Insight candidate must state a limitation or counterexample");
            }
        }

        validate_text("freshness reason", &self.freshness.reason, 1_000)?;
        if let Some(review_after) = self.freshness.review_after.as_deref() {
            DateTime::parse_from_rfc3339(review_after).map_err(|_| {
                KnowledgeEvolutionError::Invalid(
                    "freshness review_after must be an RFC3339 timestamp".to_string(),
                )
            })?;
        }

        let mut factors = BTreeSet::new();
        for factor in &self.importance {
            if !factor.score.is_finite() || !(0.0..=1.0).contains(&factor.score) {
                return invalid("importance factor score must be between zero and one");
            }
            validate_text("importance factor reason", &factor.reason, 1_000)?;
            if !factors.insert(factor.factor) {
                return invalid("candidate contains a duplicate importance factor");
            }
        }
        if factors != KnowledgeImportanceFactorKind::ALL.into_iter().collect() {
            return invalid("candidate must explain all seven importance factors");
        }

        let mut outcomes = BTreeSet::new();
        if self
            .verified_outcome_ids
            .iter()
            .any(|outcome_id| !outcomes.insert(*outcome_id))
        {
            return invalid("candidate contains a duplicate verified Outcome");
        }
        Ok(())
    }

    pub fn readiness(&self) -> KnowledgeEvolutionReadiness {
        if self.target_kind == KnowledgeEvolutionTargetKind::Insight
            && self.verified_outcome_ids.is_empty()
        {
            KnowledgeEvolutionReadiness::NeedsOutcomeEvidence
        } else {
            KnowledgeEvolutionReadiness::ReadyForReview
        }
    }

    pub fn source_ids(&self) -> Vec<Uuid> {
        self.claims
            .iter()
            .flat_map(|claim| claim.source_ids.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn importance_score(&self) -> f32 {
        self.importance
            .iter()
            .map(|factor| factor.score)
            .sum::<f32>()
            / self.importance.len() as f32
    }
}

#[derive(Debug, Error)]
pub enum KnowledgeEvolutionError {
    #[error("knowledge evolution candidate is invalid: {0}")]
    Invalid(String),
}

fn citation_source_ids(citations: &Value) -> Result<BTreeSet<Uuid>, KnowledgeEvolutionError> {
    let entries = citations.as_array().ok_or_else(|| {
        KnowledgeEvolutionError::Invalid("citations must be an array".to_string())
    })?;
    let mut sources = BTreeSet::new();
    for entry in entries {
        let source_id = entry
            .get("source_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                KnowledgeEvolutionError::Invalid(
                    "every citation must contain a source_id UUID".to_string(),
                )
            })?;
        sources.insert(source_id);
    }
    if sources.is_empty() {
        return invalid("candidate needs at least one cited Source");
    }
    Ok(sources)
}

fn validate_claim_refs<'a>(
    field: &str,
    values: &'a [String],
    claim_ids: &BTreeSet<&str>,
    required: bool,
) -> Result<BTreeSet<&'a str>, KnowledgeEvolutionError> {
    if required && values.is_empty() {
        return invalid(&format!("{field} must not be empty"));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        if !claim_ids.contains(value.as_str()) {
            return invalid(&format!("{field} references an unknown claim"));
        }
        if !unique.insert(value.as_str()) {
            return invalid(&format!("{field} contains a duplicate claim"));
        }
    }
    Ok(unique)
}

fn validate_string_list(
    field: &str,
    values: &[String],
    required: bool,
) -> Result<(), KnowledgeEvolutionError> {
    if (required && values.is_empty()) || values.len() > MAX_SCOPE_ITEMS {
        return invalid(&format!("{field} must contain 1-{MAX_SCOPE_ITEMS} items"));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        validate_text(field, value, 1_000)?;
        if !unique.insert(value.as_str()) {
            return invalid(&format!("{field} contains a duplicate item"));
        }
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> Result<(), KnowledgeEvolutionError> {
    let valid = (1..=64).contains(&value.len())
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
    if valid {
        Ok(())
    } else {
        invalid(&format!("{field} is not a valid identifier"))
    }
}

fn validate_text(field: &str, value: &str, maximum: usize) -> Result<(), KnowledgeEvolutionError> {
    if value.trim().is_empty() || value.len() > maximum {
        invalid(&format!("{field} must contain 1-{maximum} UTF-8 bytes"))
    } else {
        Ok(())
    }
}

fn invalid<T>(detail: &str) -> Result<T, KnowledgeEvolutionError> {
    Err(KnowledgeEvolutionError::Invalid(detail.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factors() -> Vec<KnowledgeImportanceFactor> {
        KnowledgeImportanceFactorKind::ALL
            .into_iter()
            .map(|factor| KnowledgeImportanceFactor {
                factor,
                score: 0.5,
                reason: "Source-backed triage signal".to_string(),
            })
            .collect()
    }

    fn lesson(source_id: Uuid) -> KnowledgeEvolutionCandidate {
        KnowledgeEvolutionCandidate {
            schema_version: 1,
            dossier_artifact_id: None,
            target_kind: KnowledgeEvolutionTargetKind::Lesson,
            claim: "Check service health before declaring recovery".to_string(),
            claims: vec![KnowledgeEvolutionClaim {
                id: "health-check".to_string(),
                statement: "The runbook requires a health check".to_string(),
                source_ids: vec![source_id],
            }],
            supporting_claim_ids: vec!["health-check".to_string()],
            contradicting_claim_ids: Vec::new(),
            applicability: vec!["Service recovery".to_string()],
            limitations: vec!["Does not identify the original fault".to_string()],
            freshness: KnowledgeFreshness {
                status: KnowledgeFreshnessStatus::Current,
                review_after: None,
                reason: "Current runbook revision".to_string(),
            },
            confidence: 0.8,
            importance: factors(),
            verified_outcome_ids: Vec::new(),
        }
    }

    #[test]
    fn structured_lesson_requires_claims_sources_and_all_importance_factors() {
        let source_id = Uuid::new_v4();
        let citations = serde_json::json!([{"source_id": source_id}]);
        let candidate = lesson(source_id);
        candidate.validate(&citations).unwrap();
        assert_eq!(
            candidate.readiness(),
            KnowledgeEvolutionReadiness::ReadyForReview
        );
        assert_eq!(candidate.source_ids(), vec![source_id]);

        let mut invalid = candidate;
        invalid.importance.pop();
        assert!(invalid.validate(&citations).is_err());
    }

    #[test]
    fn insight_without_verified_outcome_stays_a_candidate() {
        let source_id = Uuid::new_v4();
        let citations = serde_json::json!([{"source_id": source_id}]);
        let mut candidate = lesson(source_id);
        candidate.target_kind = KnowledgeEvolutionTargetKind::Insight;
        candidate.claims.push(KnowledgeEvolutionClaim {
            id: "second-source".to_string(),
            statement: "A second observation supports the generalized finding".to_string(),
            source_ids: vec![source_id],
        });
        candidate
            .supporting_claim_ids
            .push("second-source".to_string());
        candidate.validate(&citations).unwrap();
        assert_eq!(
            candidate.readiness(),
            KnowledgeEvolutionReadiness::NeedsOutcomeEvidence
        );
    }
}
