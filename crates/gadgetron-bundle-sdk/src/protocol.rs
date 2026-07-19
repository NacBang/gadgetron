use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BundleId, BundleSdkError, GadgetName, InvocationLeaseToken, LocalId, Result,
    BUNDLE_HOST_PROTOCOL_VERSION,
};

const MAX_MESSAGE_ID: usize = 128;
const MAX_OPAQUE_ID: usize = 256;
const MAX_TEXT: usize = 2_048;
const MAX_EVIDENCE_PASSAGE: usize = 32_768;
const MAX_JSON_BYTES: usize = 1_048_576;

/// Versioned identity-bound transport envelope used in both directions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProtocolEnvelope<T> {
    pub protocol_version: u32,
    pub message_id: String,
    pub bundle: BundleRuntimeIdentity,
    pub payload: T,
}

impl<T> ProtocolEnvelope<T> {
    pub fn new(message_id: impl Into<String>, bundle: BundleRuntimeIdentity, payload: T) -> Self {
        Self {
            protocol_version: BUNDLE_HOST_PROTOCOL_VERSION,
            message_id: message_id.into(),
            bundle,
            payload,
        }
    }

    /// Verify routing identity and version before dispatching the payload.
    pub fn validate_routing(
        &self,
        expected_bundle: &BundleRuntimeIdentity,
        expected_protocol: u32,
    ) -> Result<()> {
        if self.protocol_version != expected_protocol {
            return Err(BundleSdkError::protocol(
                "protocol_version",
                format!(
                    "expected {expected_protocol}, received {}",
                    self.protocol_version
                ),
            ));
        }
        validate_message_id(&self.message_id)?;
        if &self.bundle != expected_bundle {
            return Err(BundleSdkError::protocol(
                "bundle",
                format!(
                    "expected {}@{}, received {}@{}",
                    expected_bundle.id,
                    expected_bundle.version,
                    self.bundle.id,
                    self.bundle.version
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleRuntimeIdentity {
    pub id: BundleId,
    pub version: Version,
}

impl BundleRuntimeIdentity {
    pub fn new(id: BundleId, version: Version) -> Self {
        Self { id, version }
    }
}

/// Messages accepted by an external Bundle host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostRequest {
    Handshake(HandshakeRequest),
    Health(HealthRequest),
    InvokeGadget(GadgetInvocation),
    StartJob(JobStartRequest),
    PollJob(JobPollRequest),
    CancelJob(JobCancelRequest),
    Shutdown(ShutdownRequest),
}

impl HostRequest {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Handshake(request) => request.validate(),
            Self::Health(_) => Ok(()),
            Self::InvokeGadget(request) => request.validate(),
            Self::StartJob(request) => request.validate(),
            Self::PollJob(request) => validate_opaque_id("poll_job.job_id", &request.job_id),
            Self::CancelJob(request) => {
                validate_opaque_id("cancel_job.job_id", &request.job_id)?;
                if let Some(reason) = &request.reason {
                    bounded_nonempty("cancel_job.reason", reason, MAX_TEXT)?;
                }
                Ok(())
            }
            Self::Shutdown(request) => {
                if let Some(reason) = &request.reason {
                    bounded_nonempty("shutdown.reason", reason, MAX_TEXT)?;
                }
                Ok(())
            }
        }
    }
}

/// Responses emitted by an external Bundle host. Evidence, candidates and
/// outcomes in these responses remain advisory until Core accepts them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostResponse {
    Handshake(HandshakeResponse),
    Health(HealthReport),
    GadgetResult(GadgetResult),
    JobAccepted(JobAccepted),
    JobStatus(JobStatusReport),
    Acknowledgement(Acknowledgement),
    Error(HostError),
}

impl HostResponse {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Handshake(response) => response.validate(),
            Self::Health(report) => report.validate(),
            Self::GadgetResult(result) => result.validate(),
            Self::JobAccepted(job) => validate_opaque_id("job_accepted.job_id", &job.job_id),
            Self::JobStatus(job) => job.validate(),
            Self::Acknowledgement(ack) => bounded_nonempty("ack.message", &ack.message, MAX_TEXT),
            Self::Error(error) => error.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct HandshakeRequest {
    pub package_manifest_sha256: String,
    pub protocol_min: u32,
    pub protocol_max: u32,
}

impl HandshakeRequest {
    pub fn new(
        package_manifest_sha256: impl Into<String>,
        protocol_min: u32,
        protocol_max: u32,
    ) -> Self {
        Self {
            package_manifest_sha256: package_manifest_sha256.into(),
            protocol_min,
            protocol_max,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_sha256(
            "handshake.package_manifest_sha256",
            &self.package_manifest_sha256,
        )?;
        if self.protocol_min == 0 || self.protocol_min > self.protocol_max {
            return Err(BundleSdkError::protocol(
                "handshake.protocol_min",
                "protocol_min must begin at 1 and not exceed protocol_max",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct HandshakeResponse {
    pub package_manifest_sha256: String,
    pub selected_protocol: u32,
}

impl HandshakeResponse {
    pub fn new(package_manifest_sha256: impl Into<String>, selected_protocol: u32) -> Self {
        Self {
            package_manifest_sha256: package_manifest_sha256.into(),
            selected_protocol,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_sha256(
            "handshake.package_manifest_sha256",
            &self.package_manifest_sha256,
        )?;
        if self.selected_protocol == 0 {
            return Err(BundleSdkError::protocol(
                "handshake.selected_protocol",
                "selected protocol must begin at 1",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct HealthRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct HealthReport {
    pub status: HealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl HealthReport {
    pub fn healthy() -> Self {
        Self {
            status: HealthStatus::Healthy,
            message: None,
        }
    }

    pub fn with_message(status: HealthStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: Some(message.into()),
        }
    }

    fn validate(&self) -> Result<()> {
        if let Some(message) = &self.message {
            bounded_nonempty("health.message", message, MAX_TEXT)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GadgetInvocation {
    pub gadget: GadgetName,
    pub input: Value,
    pub context: InvocationContext,
}

impl GadgetInvocation {
    pub fn new(gadget: GadgetName, input: Value, context: InvocationContext) -> Self {
        Self {
            gadget,
            input,
            context,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_json_size("invoke_gadget.input", &self.input)?;
        self.context.validate()
    }
}

/// Core-authenticated request context. Secret handles are opaque references;
/// raw credentials and database URLs are deliberately absent from this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct InvocationContext {
    pub tenant_id: String,
    pub actor_id: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_space_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub secret_handles: BTreeMap<LocalId, String>,
    /// Short-lived Core-issued bearer lease for non-probe broker operations.
    /// The Bundle may present it to the broker but cannot choose its tenant,
    /// actor, scope, package digest or expiry binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_lease: Option<InvocationLeaseToken>,
}

impl InvocationContext {
    pub fn new(
        tenant_id: impl Into<String>,
        actor_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            actor_id: actor_id.into(),
            request_id: request_id.into(),
            acting_space_id: None,
            conversation_id: None,
            scopes: Vec::new(),
            secret_handles: BTreeMap::new(),
            broker_lease: None,
        }
    }

    pub fn with_conversation_id(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    pub fn with_acting_space_id(mut self, space_id: impl Into<String>) -> Self {
        self.acting_space_id = Some(space_id.into());
        self
    }

    pub fn with_scopes(mut self, scopes: impl IntoIterator<Item = String>) -> Self {
        self.scopes = scopes.into_iter().collect();
        self
    }

    pub fn with_secret_handle(mut self, id: LocalId, handle: impl Into<String>) -> Self {
        self.secret_handles.insert(id, handle.into());
        self
    }

    pub fn with_broker_lease(mut self, lease: InvocationLeaseToken) -> Self {
        self.broker_lease = Some(lease);
        self
    }

    fn validate(&self) -> Result<()> {
        validate_opaque_id("context.tenant_id", &self.tenant_id)?;
        validate_opaque_id("context.actor_id", &self.actor_id)?;
        validate_opaque_id("context.request_id", &self.request_id)?;
        if let Some(space_id) = &self.acting_space_id {
            validate_opaque_id("context.acting_space_id", space_id)?;
        }
        if let Some(conversation_id) = &self.conversation_id {
            validate_opaque_id("context.conversation_id", conversation_id)?;
        }
        ensure_unique("context.scopes", self.scopes.iter().map(String::as_str))?;
        for (index, scope) in self.scopes.iter().enumerate() {
            bounded_nonempty(&format!("context.scopes[{index}]"), scope, MAX_OPAQUE_ID)?;
        }
        for (id, handle) in &self.secret_handles {
            bounded_nonempty(
                &format!("context.secret_handles.{id}"),
                handle,
                MAX_OPAQUE_ID,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobStartRequest {
    pub recipe_id: LocalId,
    #[serde(default)]
    pub parameters: Value,
    pub context: InvocationContext,
}

impl JobStartRequest {
    pub fn new(recipe_id: LocalId, parameters: Value, context: InvocationContext) -> Self {
        Self {
            recipe_id,
            parameters,
            context,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_json_size("start_job.parameters", &self.parameters)?;
        self.context.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobPollRequest {
    pub job_id: String,
}

impl JobPollRequest {
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobCancelRequest {
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl JobCancelRequest {
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ShutdownRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ShutdownRequest {
    pub fn with_reason(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GadgetResult {
    pub output: Value,
    #[serde(default)]
    pub evidence: Vec<EvidenceSubmission>,
    #[serde(default)]
    pub candidates: Vec<CandidateSubmission>,
    #[serde(default)]
    pub outcomes: Vec<OutcomeObservation>,
}

impl GadgetResult {
    pub fn new(output: Value) -> Self {
        Self {
            output,
            evidence: Vec::new(),
            candidates: Vec::new(),
            outcomes: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence: EvidenceSubmission) -> Self {
        self.evidence.push(evidence);
        self
    }

    pub fn with_candidate(mut self, candidate: CandidateSubmission) -> Self {
        self.candidates.push(candidate);
        self
    }

    pub fn with_outcome(mut self, outcome: OutcomeObservation) -> Self {
        self.outcomes.push(outcome);
        self
    }

    fn validate(&self) -> Result<()> {
        validate_json_size("gadget_result.output", &self.output)?;
        for (index, evidence) in self.evidence.iter().enumerate() {
            evidence.validate(index)?;
        }
        for (index, candidate) in self.candidates.iter().enumerate() {
            candidate.validate(index, self.evidence.len())?;
        }
        for (index, outcome) in self.outcomes.iter().enumerate() {
            outcome.validate(index, self.evidence.len())?;
        }
        Ok(())
    }
}

/// Advisory source material. Core assigns provenance, ACL and verification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct EvidenceSubmission {
    pub source: String,
    pub passage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl EvidenceSubmission {
    pub fn new(source: impl Into<String>, passage: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            passage: passage.into(),
            observed_at: None,
            content_sha256: None,
            metadata: BTreeMap::new(),
        }
    }

    fn validate(&self, index: usize) -> Result<()> {
        bounded_nonempty(
            &format!("gadget_result.evidence[{index}].source"),
            &self.source,
            MAX_TEXT,
        )?;
        bounded_nonempty(
            &format!("gadget_result.evidence[{index}].passage"),
            &self.passage,
            MAX_EVIDENCE_PASSAGE,
        )?;
        if let Some(observed_at) = &self.observed_at {
            bounded_nonempty(
                &format!("gadget_result.evidence[{index}].observed_at"),
                observed_at,
                MAX_OPAQUE_ID,
            )?;
        }
        if let Some(digest) = &self.content_sha256 {
            validate_sha256(
                &format!("gadget_result.evidence[{index}].content_sha256"),
                digest,
            )?;
        }
        for (key, value) in &self.metadata {
            bounded_nonempty(
                &format!("gadget_result.evidence[{index}].metadata key"),
                key,
                MAX_OPAQUE_ID,
            )?;
            validate_json_size(
                &format!("gadget_result.evidence[{index}].metadata.{key}"),
                value,
            )?;
        }
        Ok(())
    }
}

/// Advisory knowledge candidate. Core decides whether and how it enters the
/// Awakening Engine's canonical graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CandidateSubmission {
    pub kind: String,
    pub data: Value,
    #[serde(default)]
    pub evidence_indices: Vec<usize>,
}

impl CandidateSubmission {
    pub fn new(kind: impl Into<String>, data: Value) -> Self {
        Self {
            kind: kind.into(),
            data,
            evidence_indices: Vec::new(),
        }
    }

    fn validate(&self, index: usize, evidence_len: usize) -> Result<()> {
        bounded_nonempty(
            &format!("gadget_result.candidates[{index}].kind"),
            &self.kind,
            MAX_OPAQUE_ID,
        )?;
        validate_json_size(
            &format!("gadget_result.candidates[{index}].data"),
            &self.data,
        )?;
        validate_evidence_indices(
            &format!("gadget_result.candidates[{index}].evidence_indices"),
            &self.evidence_indices,
            evidence_len,
        )
    }
}

/// Advisory observation of an operation's result. Core evaluates the
/// authoritative outcome predicate and any corrective action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OutcomeObservation {
    pub status: ObservedOutcome,
    pub summary: String,
    #[serde(default)]
    pub details: Value,
    #[serde(default)]
    pub evidence_indices: Vec<usize>,
}

impl OutcomeObservation {
    pub fn new(status: ObservedOutcome, summary: impl Into<String>) -> Self {
        Self {
            status,
            summary: summary.into(),
            details: Value::Null,
            evidence_indices: Vec::new(),
        }
    }

    fn validate(&self, index: usize, evidence_len: usize) -> Result<()> {
        bounded_nonempty(
            &format!("gadget_result.outcomes[{index}].summary"),
            &self.summary,
            MAX_TEXT,
        )?;
        validate_json_size(
            &format!("gadget_result.outcomes[{index}].details"),
            &self.details,
        )?;
        validate_evidence_indices(
            &format!("gadget_result.outcomes[{index}].evidence_indices"),
            &self.evidence_indices,
            evidence_len,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ObservedOutcome {
    Succeeded,
    Failed,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobAccepted {
    pub job_id: String,
}

impl JobAccepted {
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobStatusReport {
    pub job_id: String,
    pub status: JobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<GadgetResult>,
}

impl JobStatusReport {
    pub fn new(job_id: impl Into<String>, status: JobStatus) -> Self {
        Self {
            job_id: job_id.into(),
            status,
            progress: None,
            result: None,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_opaque_id("job_status.job_id", &self.job_id)?;
        if let Some(progress) = &self.progress {
            validate_json_size("job_status.progress", progress)?;
        }
        if let Some(result) = &self.result {
            result.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Acknowledgement {
    pub message: String,
}

impl Acknowledgement {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct HostError {
    pub code: LocalId,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl HostError {
    pub fn new(code: LocalId, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            details: None,
        }
    }

    fn validate(&self) -> Result<()> {
        bounded_nonempty("error.message", &self.message, MAX_TEXT)?;
        if let Some(details) = &self.details {
            validate_json_size("error.details", details)?;
        }
        Ok(())
    }
}

fn validate_message_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_MESSAGE_ID
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(BundleSdkError::protocol(
            "message_id",
            "must contain 1-128 characters from [A-Za-z0-9_.:-]",
        ));
    }
    Ok(())
}

fn validate_opaque_id(field: &str, value: &str) -> Result<()> {
    bounded_nonempty(field, value, MAX_OPAQUE_ID)
}

fn bounded_nonempty(field: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(BundleSdkError::protocol(
            field,
            format!("must contain 1-{max} characters and no control characters"),
        ));
    }
    Ok(())
}

fn validate_sha256(field: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(BundleSdkError::protocol(
            field,
            "must be a 64-character lowercase hexadecimal SHA-256 digest",
        ));
    }
    Ok(())
}

fn validate_json_size(field: &str, value: &Value) -> Result<()> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        BundleSdkError::protocol(field, format!("JSON cannot be serialized: {error}"))
    })?;
    if encoded.len() > MAX_JSON_BYTES {
        return Err(BundleSdkError::protocol(
            field,
            format!("JSON exceeds {MAX_JSON_BYTES} serialized bytes"),
        ));
    }
    Ok(())
}

fn ensure_unique<'a>(field: &str, values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(BundleSdkError::protocol(
                field,
                format!("duplicate value {value:?}"),
            ));
        }
    }
    Ok(())
}

fn validate_evidence_indices(field: &str, indices: &[usize], evidence_len: usize) -> Result<()> {
    let mut seen = BTreeSet::new();
    for index in indices {
        if *index >= evidence_len {
            return Err(BundleSdkError::protocol(
                field,
                format!("index {index} is outside evidence array length {evidence_len}"),
            ));
        }
        if !seen.insert(*index) {
            return Err(BundleSdkError::protocol(
                field,
                format!("duplicate evidence index {index}"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> BundleRuntimeIdentity {
        BundleRuntimeIdentity::new(
            BundleId::new("server-administrator").unwrap(),
            Version::new(1, 0, 0),
        )
    }

    #[test]
    fn request_envelope_round_trips_with_stable_method_tag() {
        let context = InvocationContext::new("tenant-1", "manager-1", "request-1")
            .with_acting_space_id("11111111-1111-1111-1111-111111111111")
            .with_scopes(vec!["ServerRead".to_string()])
            .with_secret_handle(LocalId::new("ssh-key").unwrap(), "secret-ref:opaque-42");
        let request = ProtocolEnvelope::new(
            "message-1",
            identity(),
            HostRequest::InvokeGadget(GadgetInvocation::new(
                GadgetName::new("server.inventory-list").unwrap(),
                serde_json::json!({"limit": 10}),
                context,
            )),
        );
        request
            .validate_routing(&identity(), BUNDLE_HOST_PROTOCOL_VERSION)
            .unwrap();
        request.payload.validate().unwrap();

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"method\":\"invoke_gadget\""));
        assert!(json.contains("\"acting_space_id\":\"11111111-1111-1111-1111-111111111111\""));
        let decoded: ProtocolEnvelope<HostRequest> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn response_submissions_are_advisory_and_cross_references_validate() {
        let mut candidate =
            CandidateSubmission::new("server.host", serde_json::json!({"hostname": "edge-1"}));
        candidate.evidence_indices.push(0);
        let response = ProtocolEnvelope::new(
            "message-2",
            identity(),
            HostResponse::GadgetResult(
                GadgetResult::new(serde_json::json!({"count": 1}))
                    .with_evidence(EvidenceSubmission::new(
                        "ssh://edge-1/uname",
                        "Linux edge-1",
                    ))
                    .with_candidate(candidate)
                    .with_outcome(OutcomeObservation::new(
                        ObservedOutcome::Succeeded,
                        "inventory observed",
                    )),
            ),
        );
        response.payload.validate().unwrap();
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("approved"));
        assert!(!json.contains("verified"));
        let decoded: ProtocolEnvelope<HostResponse> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn raw_credentials_are_not_part_of_invocation_wire_shape() {
        let json = r#"{
            "method":"invoke_gadget",
            "params":{
                "gadget":"server.inventory-list",
                "input":{},
                "context":{
                    "tenant_id":"tenant-1",
                    "actor_id":"manager-1",
                    "request_id":"request-1",
                    "api_key":"raw-secret"
                }
            }
        }"#;
        assert!(serde_json::from_str::<HostRequest>(json).is_err());
    }

    #[test]
    fn routing_rejects_identity_and_protocol_mismatch() {
        let envelope = ProtocolEnvelope::new(
            "message-3",
            identity(),
            HostRequest::Health(HealthRequest::default()),
        );
        let other = BundleRuntimeIdentity::new(
            BundleId::new("restaurant-research").unwrap(),
            Version::new(1, 0, 0),
        );
        assert!(envelope.validate_routing(&other, 1).is_err());
        assert!(envelope.validate_routing(&identity(), 2).is_err());
    }
}
