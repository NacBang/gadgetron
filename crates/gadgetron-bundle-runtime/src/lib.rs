//! Domain-neutral async helpers for external Bundle runtimes.
//!
//! This crate owns wire mechanics only. Core remains authoritative for tenant
//! identity, leases, permissions, policy, database execution and secrets.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use gadgetron_bundle_sdk::{
    Acknowledgement, BrokerEnvelope, BrokerError, BrokerProbeRequest, BrokerRequest,
    BrokerResource, BrokerResponse, BundleRuntimeIdentity, DatabaseDeleteRequest,
    DatabaseInsertRequest, DatabaseMutationResult, DatabaseRows, DatabaseSelectRequest,
    DatabaseUpdateRequest, GadgetInvocation, HandshakeResponse, HealthReport, HealthStatus,
    HostError, HostRequest, HostResponse, IntelligenceContextRequest, KnowledgeCollectionRequest,
    KnowledgeCollectionResult, KnowledgeContextPack, LocalId, OutcomeFeedbackReceipt,
    OutcomeFeedbackRequest, ProtocolEnvelope, SshExecuteRequest, SshExecutionResult,
    BUNDLE_HOST_PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Mutex,
    time::timeout,
};

pub const DEFAULT_MAX_FRAME_BYTES: usize = 1_048_576;
pub const DEFAULT_BROKER_TIMEOUT: Duration = Duration::from_secs(10);

type BoxAsyncRead = Box<dyn AsyncRead + Unpin + Send>;
type BoxAsyncWrite = Box<dyn AsyncWrite + Unpin + Send>;

/// Host runtime for signed packages whose executable behavior is entirely
/// described by Core-owned collection, ontology, and Knowledge role contracts.
pub struct ManifestBundleRuntime {
    identity: BundleRuntimeIdentity,
    manifest_sha256: String,
    health_message: String,
    handshaken: bool,
}

impl ManifestBundleRuntime {
    pub fn new(
        identity: BundleRuntimeIdentity,
        manifest_sha256: impl Into<String>,
        health_message: impl Into<String>,
    ) -> Result<Self, ManifestRuntimeError> {
        let manifest_sha256 = manifest_sha256.into();
        if manifest_sha256.len() != 64
            || !manifest_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(ManifestRuntimeError::InvalidManifestDigest);
        }
        Ok(Self {
            identity,
            manifest_sha256,
            health_message: health_message.into(),
            handshaken: false,
        })
    }

    pub fn identity(&self) -> &BundleRuntimeIdentity {
        &self.identity
    }

    pub async fn serve<R, W>(
        &mut self,
        reader: R,
        mut writer: W,
    ) -> Result<(), ManifestRuntimeError>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut reader = BufReader::new(reader);
        loop {
            let Some(frame) = read_manifest_frame(&mut reader, DEFAULT_MAX_FRAME_BYTES).await?
            else {
                return Ok(());
            };
            let request: ProtocolEnvelope<HostRequest> = serde_json::from_slice(&frame)?;
            request.validate_routing(&self.identity, BUNDLE_HOST_PROTOCOL_VERSION)?;
            request.payload.validate()?;
            let (payload, stop) = self.handle(request.payload);
            let response =
                ProtocolEnvelope::new(request.message_id, self.identity.clone(), payload);
            response.validate_routing(&self.identity, BUNDLE_HOST_PROTOCOL_VERSION)?;
            response.payload.validate()?;
            let mut encoded = serde_json::to_vec(&response)?;
            if encoded.len() >= DEFAULT_MAX_FRAME_BYTES {
                return Err(ManifestRuntimeError::FrameTooLarge);
            }
            encoded.push(b'\n');
            writer.write_all(&encoded).await?;
            writer.flush().await?;
            if stop {
                return Ok(());
            }
        }
    }

    fn handle(&mut self, request: HostRequest) -> (HostResponse, bool) {
        match request {
            HostRequest::Handshake(handshake) => {
                if handshake.package_manifest_sha256 != self.manifest_sha256 {
                    return (
                        manifest_host_error(
                            "manifest-digest-mismatch",
                            "runtime manifest digest does not match the selected package",
                        ),
                        false,
                    );
                }
                if handshake.protocol_min > BUNDLE_HOST_PROTOCOL_VERSION
                    || handshake.protocol_max < BUNDLE_HOST_PROTOCOL_VERSION
                {
                    return (
                        manifest_host_error(
                            "protocol-not-supported",
                            "runtime and Core do not share a host protocol version",
                        ),
                        false,
                    );
                }
                self.handshaken = true;
                (
                    HostResponse::Handshake(HandshakeResponse::new(
                        self.manifest_sha256.clone(),
                        BUNDLE_HOST_PROTOCOL_VERSION,
                    )),
                    false,
                )
            }
            HostRequest::Shutdown(_) if self.handshaken => (
                HostResponse::Acknowledgement(Acknowledgement::new("Bundle runtime stopping")),
                true,
            ),
            _ if !self.handshaken => (
                manifest_host_error(
                    "handshake-required",
                    "complete the package-bound handshake before using the runtime",
                ),
                false,
            ),
            HostRequest::Health(_) => (
                HostResponse::Health(HealthReport::with_message(
                    HealthStatus::Healthy,
                    self.health_message.clone(),
                )),
                false,
            ),
            HostRequest::Shutdown(_) => unreachable!("handshake guard handles shutdown"),
            _ => (
                manifest_host_error(
                    "request-not-supported",
                    "this package exposes signed Core-owned Knowledge contracts only",
                ),
                false,
            ),
        }
    }
}

#[derive(Debug, Error)]
pub enum ManifestRuntimeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid SDK contract: {0}")]
    Sdk(#[from] gadgetron_bundle_sdk::BundleSdkError),
    #[error("manifest SHA-256 must be exactly 64 lowercase hexadecimal characters")]
    InvalidManifestDigest,
    #[error("protocol frame is larger than the runtime byte ceiling")]
    FrameTooLarge,
    #[error("protocol frame ended without a newline")]
    UnterminatedFrame,
}

async fn read_manifest_frame<R>(
    reader: &mut BufReader<R>,
    maximum: usize,
) -> Result<Option<Vec<u8>>, ManifestRuntimeError>
where
    R: AsyncRead + Unpin,
{
    let mut frame = Vec::new();
    let mut limited = reader.take((maximum + 1) as u64);
    let read = limited.read_until(b'\n', &mut frame).await?;
    if read == 0 {
        return Ok(None);
    }
    if frame.len() > maximum {
        return Err(ManifestRuntimeError::FrameTooLarge);
    }
    if frame.pop() != Some(b'\n') {
        return Err(ManifestRuntimeError::UnterminatedFrame);
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Ok(Some(frame))
}

fn manifest_host_error(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(HostError::new(
        LocalId::new(code).expect("static host error code is canonical"),
        message,
        false,
    ))
}

pub type SharedBundleBroker = Arc<Mutex<BundleBrokerClient>>;

#[async_trait]
pub trait BundleGadgetHandler: Send + Sync {
    async fn health(&self, broker: &SharedBundleBroker) -> HealthReport;

    async fn invoke(
        &self,
        invocation: GadgetInvocation,
        broker: &SharedBundleBroker,
    ) -> HostResponse;
}

/// Shared host lifecycle for independently shipped Bundles that execute
/// Gadgets through the typed Core broker.
pub struct GadgetBundleRuntime<H> {
    identity: BundleRuntimeIdentity,
    manifest_sha256: String,
    display_name: String,
    handshaken: bool,
    broker: Option<SharedBundleBroker>,
    handler: H,
}

impl<H> GadgetBundleRuntime<H>
where
    H: BundleGadgetHandler,
{
    pub fn new(
        identity: BundleRuntimeIdentity,
        manifest_sha256: impl Into<String>,
        display_name: impl Into<String>,
        handler: H,
    ) -> Result<Self, GadgetRuntimeError> {
        let manifest_sha256 = manifest_sha256.into();
        if manifest_sha256.len() != 64
            || !manifest_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(GadgetRuntimeError::InvalidManifestDigest);
        }
        Ok(Self {
            identity,
            manifest_sha256,
            display_name: display_name.into(),
            handshaken: false,
            broker: None,
            handler,
        })
    }

    pub fn identity(&self) -> &BundleRuntimeIdentity {
        &self.identity
    }

    pub fn with_broker(mut self, broker: BundleBrokerClient) -> Self {
        self.broker = Some(Arc::new(Mutex::new(broker)));
        self
    }

    pub async fn serve<R, W>(&mut self, reader: R, mut writer: W) -> Result<(), GadgetRuntimeError>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut reader = BufReader::new(reader);
        loop {
            let Some(frame) = read_gadget_frame(&mut reader, DEFAULT_MAX_FRAME_BYTES).await? else {
                return Ok(());
            };
            let request: ProtocolEnvelope<HostRequest> = serde_json::from_slice(&frame)?;
            request.validate_routing(&self.identity, BUNDLE_HOST_PROTOCOL_VERSION)?;
            request.payload.validate()?;
            let (payload, stop) = self.handle(request.payload).await;
            let response =
                ProtocolEnvelope::new(request.message_id, self.identity.clone(), payload);
            response.validate_routing(&self.identity, BUNDLE_HOST_PROTOCOL_VERSION)?;
            response.payload.validate()?;
            let mut encoded = serde_json::to_vec(&response)?;
            if encoded.len() >= DEFAULT_MAX_FRAME_BYTES {
                return Err(GadgetRuntimeError::FrameTooLarge);
            }
            encoded.push(b'\n');
            writer.write_all(&encoded).await?;
            writer.flush().await?;
            if stop {
                return Ok(());
            }
        }
    }

    async fn handle(&mut self, request: HostRequest) -> (HostResponse, bool) {
        match request {
            HostRequest::Handshake(handshake) => {
                if handshake.package_manifest_sha256 != self.manifest_sha256 {
                    return (
                        gadget_host_error(
                            "manifest-digest-mismatch",
                            "runtime manifest digest does not match the selected package",
                        ),
                        false,
                    );
                }
                if handshake.protocol_min > BUNDLE_HOST_PROTOCOL_VERSION
                    || handshake.protocol_max < BUNDLE_HOST_PROTOCOL_VERSION
                {
                    return (
                        gadget_host_error(
                            "protocol-not-supported",
                            "runtime and Core do not share a host protocol version",
                        ),
                        false,
                    );
                }
                self.handshaken = true;
                (
                    HostResponse::Handshake(HandshakeResponse::new(
                        self.manifest_sha256.clone(),
                        BUNDLE_HOST_PROTOCOL_VERSION,
                    )),
                    false,
                )
            }
            HostRequest::Shutdown(_) if self.handshaken => (
                HostResponse::Acknowledgement(Acknowledgement::new(format!(
                    "{} stopping",
                    self.display_name
                ))),
                true,
            ),
            _ if !self.handshaken => (
                gadget_host_error(
                    "handshake-required",
                    "complete the package-bound handshake before using the runtime",
                ),
                false,
            ),
            HostRequest::Health(_) => {
                let Some(broker) = &self.broker else {
                    return (
                        HostResponse::Health(HealthReport::with_message(
                            HealthStatus::Degraded,
                            "Core broker channel is unavailable",
                        )),
                        false,
                    );
                };
                (
                    HostResponse::Health(self.handler.health(broker).await),
                    false,
                )
            }
            HostRequest::InvokeGadget(invocation) => {
                let Some(broker) = &self.broker else {
                    return (
                        gadget_host_error(
                            "broker-unavailable",
                            "Core broker channel is unavailable",
                        ),
                        false,
                    );
                };
                (self.handler.invoke(invocation, broker).await, false)
            }
            HostRequest::Shutdown(_) => unreachable!("handshake guard handles shutdown"),
            _ => (
                gadget_host_error(
                    "request-not-supported",
                    "this Bundle supports health and Gadget invocation only",
                ),
                false,
            ),
        }
    }
}

#[derive(Debug, Error)]
pub enum GadgetRuntimeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid SDK contract: {0}")]
    Sdk(#[from] gadgetron_bundle_sdk::BundleSdkError),
    #[error("manifest SHA-256 must be exactly 64 lowercase hexadecimal characters")]
    InvalidManifestDigest,
    #[error("protocol frame is larger than the runtime byte ceiling")]
    FrameTooLarge,
    #[error("protocol frame ended without a newline")]
    UnterminatedFrame,
}

pub fn gadget_host_error(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(HostError::new(
        LocalId::new(code).expect("static host error code is canonical"),
        message,
        false,
    ))
}

pub fn broker_host_error(error: BrokerClientError) -> HostResponse {
    match error {
        BrokerClientError::Remote(error) => {
            HostResponse::Error(HostError::new(error.code, error.message, error.retryable))
        }
        error => gadget_host_error("broker-channel-failed", &error.public_message()),
    }
}

async fn read_gadget_frame<R>(
    reader: &mut BufReader<R>,
    maximum: usize,
) -> Result<Option<Vec<u8>>, GadgetRuntimeError>
where
    R: AsyncRead + Unpin,
{
    let mut frame = Vec::new();
    let mut limited = reader.take((maximum + 1) as u64);
    let read = limited.read_until(b'\n', &mut frame).await?;
    if read == 0 {
        return Ok(None);
    }
    if frame.len() > maximum {
        return Err(GadgetRuntimeError::FrameTooLarge);
    }
    if frame.pop() != Some(b'\n') {
        return Err(GadgetRuntimeError::UnterminatedFrame);
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Ok(Some(frame))
}

/// Correlated, bounded client for the fixed Bundle-to-Core broker channel.
pub struct BundleBrokerClient {
    reader: BufReader<BoxAsyncRead>,
    writer: BoxAsyncWrite,
    identity: BundleRuntimeIdentity,
    next_message_id: u64,
    max_frame_bytes: usize,
    request_timeout: Duration,
}

impl BundleBrokerClient {
    pub fn attach<S>(channel: S, identity: BundleRuntimeIdentity) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(channel);
        Self {
            reader: BufReader::new(Box::new(reader)),
            writer: Box::new(writer),
            identity,
            next_message_id: 1,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            request_timeout: DEFAULT_BROKER_TIMEOUT,
        }
    }

    pub fn with_limits(mut self, max_frame_bytes: usize, request_timeout: Duration) -> Self {
        self.max_frame_bytes = max_frame_bytes.max(1);
        self.request_timeout = request_timeout;
        self
    }

    pub async fn probe(
        &mut self,
        permission_id: LocalId,
        resource: BrokerResource,
    ) -> Result<gadgetron_bundle_sdk::BrokerProbeResult, BrokerClientError> {
        match self
            .exchange(BrokerRequest::Probe(BrokerProbeRequest::new(
                permission_id,
                resource,
            )))
            .await?
        {
            BrokerResponse::Probe(result) => Ok(result),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("probe", &response)),
        }
    }

    pub async fn database_select(
        &mut self,
        request: DatabaseSelectRequest,
    ) -> Result<DatabaseRows, BrokerClientError> {
        match self
            .exchange(BrokerRequest::DatabaseSelect(request))
            .await?
        {
            BrokerResponse::DatabaseRows(rows) => Ok(rows),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("database_rows", &response)),
        }
    }

    pub async fn database_insert(
        &mut self,
        request: DatabaseInsertRequest,
    ) -> Result<DatabaseMutationResult, BrokerClientError> {
        self.database_mutation(BrokerRequest::DatabaseInsert(request))
            .await
    }

    pub async fn database_update(
        &mut self,
        request: DatabaseUpdateRequest,
    ) -> Result<DatabaseMutationResult, BrokerClientError> {
        self.database_mutation(BrokerRequest::DatabaseUpdate(request))
            .await
    }

    pub async fn database_delete(
        &mut self,
        request: DatabaseDeleteRequest,
    ) -> Result<DatabaseMutationResult, BrokerClientError> {
        self.database_mutation(BrokerRequest::DatabaseDelete(request))
            .await
    }

    async fn database_mutation(
        &mut self,
        request: BrokerRequest,
    ) -> Result<DatabaseMutationResult, BrokerClientError> {
        match self.exchange(request).await? {
            BrokerResponse::DatabaseMutation(result) => Ok(result),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("database_mutation", &response)),
        }
    }

    pub async fn ssh_execute(
        &mut self,
        request: SshExecuteRequest,
    ) -> Result<SshExecutionResult, BrokerClientError> {
        match self.exchange(BrokerRequest::SshExecute(request)).await? {
            BrokerResponse::SshExecution(result) => Ok(result),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("ssh_execution", &response)),
        }
    }

    pub async fn intelligence_context(
        &mut self,
        request: IntelligenceContextRequest,
    ) -> Result<KnowledgeContextPack, BrokerClientError> {
        match self
            .exchange(BrokerRequest::IntelligenceContext(request))
            .await?
        {
            BrokerResponse::KnowledgeContext(result) => Ok(result),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("knowledge_context", &response)),
        }
    }

    pub async fn outcome_feedback(
        &mut self,
        request: OutcomeFeedbackRequest,
    ) -> Result<OutcomeFeedbackReceipt, BrokerClientError> {
        match self
            .exchange(BrokerRequest::OutcomeFeedback(request))
            .await?
        {
            BrokerResponse::OutcomeFeedbackAccepted(result) => Ok(result),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("outcome_feedback_accepted", &response)),
        }
    }

    pub async fn knowledge_collection(
        &mut self,
        request: KnowledgeCollectionRequest,
    ) -> Result<KnowledgeCollectionResult, BrokerClientError> {
        match self
            .exchange(BrokerRequest::KnowledgeCollection(request))
            .await?
        {
            BrokerResponse::KnowledgeCollection(result) => Ok(result),
            BrokerResponse::Error(error) => Err(BrokerClientError::Remote(error)),
            response => Err(unexpected("knowledge_collection", &response)),
        }
    }

    async fn exchange(
        &mut self,
        payload: BrokerRequest,
    ) -> Result<BrokerResponse, BrokerClientError> {
        payload.validate()?;
        let message_id = format!("{}:{}", self.identity.id, self.next_message_id);
        self.next_message_id = self.next_message_id.saturating_add(1);
        let request = BrokerEnvelope::new(message_id.clone(), self.identity.clone(), payload);
        request.validate_routing(&self.identity)?;
        let mut encoded = serde_json::to_vec(&request)?;
        if encoded.len() >= self.max_frame_bytes {
            return Err(BrokerClientError::FrameTooLarge {
                actual: encoded.len(),
                maximum: self.max_frame_bytes,
            });
        }
        encoded.push(b'\n');
        self.writer.write_all(&encoded).await?;
        self.writer.flush().await?;

        let frame = timeout(
            self.request_timeout,
            read_frame(&mut self.reader, self.max_frame_bytes),
        )
        .await
        .map_err(|_| BrokerClientError::Timeout(self.request_timeout))??;
        let response: BrokerEnvelope<BrokerResponse> = serde_json::from_slice(&frame)?;
        response.validate_routing(&self.identity)?;
        if response.message_id != message_id {
            return Err(BrokerClientError::MessageIdMismatch {
                expected: message_id,
                actual: response.message_id,
            });
        }
        response.payload.validate()?;
        Ok(response.payload)
    }
}

#[derive(Debug, Error)]
pub enum BrokerClientError {
    #[error("I/O failed")]
    Io(#[from] std::io::Error),
    #[error("invalid JSON")]
    Json(#[from] serde_json::Error),
    #[error("invalid SDK contract")]
    Sdk(#[from] gadgetron_bundle_sdk::BundleSdkError),
    #[error("frame is {actual} bytes; maximum is {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("broker closed its channel")]
    EndOfStream,
    #[error("broker frame was not newline terminated")]
    UnterminatedFrame,
    #[error("broker timed out after {0:?}")]
    Timeout(Duration),
    #[error("broker response message id mismatch")]
    MessageIdMismatch { expected: String, actual: String },
    #[error("broker returned {actual} while {expected} was required")]
    UnexpectedResponse {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("broker denied the request")]
    Remote(BrokerError),
}

impl BrokerClientError {
    /// Redacted failure text safe for a Bundle host response.
    pub fn public_message(&self) -> String {
        match self {
            Self::Remote(error) => format!("{}: {}", error.code, error.message),
            Self::Timeout(_) => "broker request timed out".into(),
            Self::FrameTooLarge { .. } => "broker frame exceeded its byte ceiling".into(),
            Self::EndOfStream => "broker channel closed".into(),
            Self::UnterminatedFrame | Self::Json(_) | Self::Sdk(_) => {
                "broker returned an invalid frame".into()
            }
            Self::MessageIdMismatch { .. } => "broker response correlation failed".into(),
            Self::UnexpectedResponse { .. } => "broker returned an unexpected response".into(),
            Self::Io(_) => "broker channel I/O failed".into(),
        }
    }
}

async fn read_frame<R>(
    reader: &mut BufReader<R>,
    maximum: usize,
) -> Result<Vec<u8>, BrokerClientError>
where
    R: AsyncRead + Unpin,
{
    let mut frame = Vec::new();
    let mut limited = reader.take((maximum + 1) as u64);
    let read = limited.read_until(b'\n', &mut frame).await?;
    if read == 0 {
        return Err(BrokerClientError::EndOfStream);
    }
    if frame.len() > maximum {
        return Err(BrokerClientError::FrameTooLarge {
            actual: frame.len(),
            maximum,
        });
    }
    if frame.pop() != Some(b'\n') {
        return Err(BrokerClientError::UnterminatedFrame);
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Ok(frame)
}

fn unexpected(expected: &'static str, response: &BrokerResponse) -> BrokerClientError {
    let actual = match response {
        BrokerResponse::Probe(_) => "probe",
        BrokerResponse::DatabaseRows(_) => "database_rows",
        BrokerResponse::DatabaseMutation(_) => "database_mutation",
        BrokerResponse::SshExecution(_) => "ssh_execution",
        BrokerResponse::KnowledgeContext(_) => "knowledge_context",
        BrokerResponse::OutcomeFeedbackAccepted(_) => "outcome_feedback_accepted",
        BrokerResponse::KnowledgeCollection(_) => "knowledge_collection",
        BrokerResponse::Error(_) => "error",
        _ => "unknown",
    };
    BrokerClientError::UnexpectedResponse { expected, actual }
}

#[cfg(test)]
mod manifest_runtime_tests {
    use gadgetron_bundle_sdk::{
        BundleId, BundleRuntimeIdentity, HandshakeRequest, HealthRequest, HostRequest,
        HostResponse, ShutdownRequest, BUNDLE_HOST_PROTOCOL_VERSION,
    };
    use semver::Version;

    use super::ManifestBundleRuntime;

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn manifest_runtime_requires_a_bound_handshake_and_exposes_health_only() {
        let identity = BundleRuntimeIdentity::new(
            BundleId::new("example-intelligence").unwrap(),
            Version::new(0, 1, 0),
        );
        let mut runtime =
            ManifestBundleRuntime::new(identity, DIGEST, "Knowledge contracts ready").unwrap();

        let (before_handshake, stop) =
            runtime.handle(HostRequest::Health(HealthRequest::default()));
        assert!(!stop);
        assert!(matches!(
            before_handshake,
            HostResponse::Error(ref error) if error.code.as_str() == "handshake-required"
        ));

        let (handshake, stop) = runtime.handle(HostRequest::Handshake(HandshakeRequest::new(
            DIGEST,
            BUNDLE_HOST_PROTOCOL_VERSION,
            BUNDLE_HOST_PROTOCOL_VERSION,
        )));
        assert!(!stop);
        assert!(matches!(handshake, HostResponse::Handshake(_)));

        let (health, stop) = runtime.handle(HostRequest::Health(HealthRequest::default()));
        assert!(!stop);
        assert!(matches!(
            health,
            HostResponse::Health(ref report)
                if report.status == gadgetron_bundle_sdk::HealthStatus::Healthy
                    && report.message.as_deref() == Some("Knowledge contracts ready")
        ));

        let (shutdown, stop) = runtime.handle(HostRequest::Shutdown(ShutdownRequest::default()));
        assert!(stop);
        assert!(matches!(shutdown, HostResponse::Acknowledgement(_)));
    }
}
