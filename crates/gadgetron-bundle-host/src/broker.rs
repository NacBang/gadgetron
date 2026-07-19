use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use gadgetron_bundle_sdk::{
    BrokerEnvelope, BrokerError, BrokerRequest, BrokerResponse, BundleRuntimeIdentity, LocalId,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    time::timeout,
};

use crate::ValidatedPackageContract;

pub const DEFAULT_BROKER_FRAME_BYTES: usize = 1_048_576;
pub const DEFAULT_BROKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_BROKER_MESSAGE_ID_WINDOW: usize = 4_096;

/// Core-authenticated caller metadata. None of these fields are accepted from
/// the Bundle wire request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerCaller {
    identity: BundleRuntimeIdentity,
    package_manifest_sha256: String,
}

impl BrokerCaller {
    pub fn from_package(package: &ValidatedPackageContract) -> Self {
        Self {
            identity: package.runtime_identity(),
            package_manifest_sha256: package.manifest_sha256().to_string(),
        }
    }

    pub fn identity(&self) -> &BundleRuntimeIdentity {
        &self.identity
    }

    pub fn package_manifest_sha256(&self) -> &str {
        &self.package_manifest_sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrokerChannelLimits {
    request_timeout: Duration,
    max_frame_bytes: usize,
    message_id_window: usize,
}

impl BrokerChannelLimits {
    pub fn new(
        request_timeout: Duration,
        max_frame_bytes: usize,
        message_id_window: usize,
    ) -> Self {
        Self {
            request_timeout,
            max_frame_bytes: max_frame_bytes.max(1),
            message_id_window: message_id_window.max(1),
        }
    }
}

impl Default for BrokerChannelLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_BROKER_REQUEST_TIMEOUT,
            DEFAULT_BROKER_FRAME_BYTES,
            DEFAULT_BROKER_MESSAGE_ID_WINDOW,
        )
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BrokerHostError {
    #[error("Bundle broker I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bundle broker frame contained invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Bundle broker contract validation failed: {0}")]
    Contract(#[from] gadgetron_bundle_sdk::BundleSdkError),

    #[error("Bundle broker frame is {actual} bytes; maximum is {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },

    #[error("Bundle broker frame was not newline terminated")]
    UnterminatedFrame,

    #[error("Bundle broker reused message id {0:?}")]
    DuplicateMessageId(String),

    #[error("Bundle broker handler timed out after {0:?}")]
    HandlerTimeout(Duration),

    #[error("Bundle broker returned {actual} for a request that requires {expected}")]
    UnexpectedResponse {
        expected: &'static str,
        actual: &'static str,
    },

    #[error("Bundle broker probe response did not match its requested permission and resource")]
    ProbeCorrelationMismatch,

    #[error("Bundle broker SSH response did not match its requested target and operation")]
    SshCorrelationMismatch,
}

/// Policy executor behind the transport boundary. Implementations must apply
/// signed request ∩ digest-pinned operator grant ∩ deployment ceiling ∩
/// invocation lease. The transport itself never upgrades a manifest request to
/// a grant.
#[async_trait]
pub trait BundleBroker: Send + Sync {
    async fn handle(&self, caller: &BrokerCaller, request: BrokerRequest) -> BrokerResponse;
}

/// Safe production default until an explicit policy executor is composed.
#[derive(Debug, Default)]
pub struct DenyAllBundleBroker;

#[async_trait]
impl BundleBroker for DenyAllBundleBroker {
    async fn handle(&self, _caller: &BrokerCaller, _request: BrokerRequest) -> BrokerResponse {
        BrokerResponse::Error(BrokerError::new(
            LocalId::new("broker-not-configured").expect("static broker error id is valid"),
            "Core has no operator-granted broker policy for this Bundle",
            false,
        ))
    }
}

/// Serve Bundle-initiated requests on a supervisor-owned, private full-duplex
/// channel. Requests are intentionally processed one at a time, which makes the
/// maximum number of in-flight operations exactly one. A bounded rolling window
/// rejects recent duplicate message ids without expiring a healthy long-lived
/// runtime or retaining every id for its lifetime.
pub async fn serve_broker_channel<S>(
    channel: S,
    caller: BrokerCaller,
    broker: Arc<dyn BundleBroker>,
    limits: BrokerChannelLimits,
) -> Result<(), BrokerHostError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(channel);
    let mut reader = BufReader::new(reader);
    let mut seen_message_ids = BTreeSet::new();
    let mut message_id_order = VecDeque::new();

    loop {
        let Some(frame) = read_frame(&mut reader, limits.max_frame_bytes).await? else {
            return Ok(());
        };
        let request: BrokerEnvelope<BrokerRequest> = serde_json::from_slice(&frame)?;
        request.validate_routing(caller.identity())?;
        request.payload.validate()?;
        if !seen_message_ids.insert(request.message_id.clone()) {
            return Err(BrokerHostError::DuplicateMessageId(request.message_id));
        }
        message_id_order.push_back(request.message_id.clone());
        if message_id_order.len() > limits.message_id_window {
            if let Some(expired) = message_id_order.pop_front() {
                seen_message_ids.remove(&expired);
            }
        }

        let message_id = request.message_id;
        let request_payload = request.payload;
        let response_payload = timeout(
            limits.request_timeout,
            broker.handle(&caller, request_payload.clone()),
        )
        .await
        .map_err(|_| BrokerHostError::HandlerTimeout(limits.request_timeout))?;
        response_payload.validate()?;
        validate_response_for_request(&request_payload, &response_payload)?;

        let response = BrokerEnvelope::new(message_id, caller.identity().clone(), response_payload);
        response.validate_routing(caller.identity())?;
        let mut encoded = serde_json::to_vec(&response)?;
        if encoded.len() >= limits.max_frame_bytes {
            return Err(BrokerHostError::FrameTooLarge {
                actual: encoded.len(),
                maximum: limits.max_frame_bytes,
            });
        }
        encoded.push(b'\n');
        writer.write_all(&encoded).await?;
        writer.flush().await?;
    }
}

fn validate_response_for_request(
    request: &BrokerRequest,
    response: &BrokerResponse,
) -> Result<(), BrokerHostError> {
    if matches!(response, BrokerResponse::Error(_)) {
        return Ok(());
    }
    match (request, response) {
        (BrokerRequest::Probe(request), BrokerResponse::Probe(response)) => {
            if request.permission_id != response.permission_id
                || request.resource != response.resource
            {
                return Err(BrokerHostError::ProbeCorrelationMismatch);
            }
            Ok(())
        }
        (BrokerRequest::DatabaseSelect(_), BrokerResponse::DatabaseRows(_)) => Ok(()),
        (
            BrokerRequest::DatabaseInsert(_)
            | BrokerRequest::DatabaseUpdate(_)
            | BrokerRequest::DatabaseDelete(_),
            BrokerResponse::DatabaseMutation(_),
        ) => Ok(()),
        (BrokerRequest::SshExecute(request), BrokerResponse::SshExecution(response)) => {
            if request.target_id != response.target_id
                || request.operation_id != response.operation_id
            {
                return Err(BrokerHostError::SshCorrelationMismatch);
            }
            Ok(())
        }
        (
            BrokerRequest::IntelligenceContext(request),
            BrokerResponse::KnowledgeContext(response),
        ) if request.draft.query_id == response.query_id
            && request.draft.subject == response.subject =>
        {
            Ok(())
        }
        (
            BrokerRequest::OutcomeFeedback(request),
            BrokerResponse::OutcomeFeedbackAccepted(response),
        ) if request.draft.feedback_id == response.feedback_id => Ok(()),
        (BrokerRequest::KnowledgeCollection(_), BrokerResponse::KnowledgeCollection(_)) => Ok(()),
        (BrokerRequest::Probe(_), response) => Err(BrokerHostError::UnexpectedResponse {
            expected: "probe",
            actual: response_name(response),
        }),
        (BrokerRequest::DatabaseSelect(_), response) => Err(BrokerHostError::UnexpectedResponse {
            expected: "database_rows",
            actual: response_name(response),
        }),
        (
            BrokerRequest::DatabaseInsert(_)
            | BrokerRequest::DatabaseUpdate(_)
            | BrokerRequest::DatabaseDelete(_),
            response,
        ) => Err(BrokerHostError::UnexpectedResponse {
            expected: "database_mutation",
            actual: response_name(response),
        }),
        (BrokerRequest::SshExecute(_), response) => Err(BrokerHostError::UnexpectedResponse {
            expected: "ssh_execution",
            actual: response_name(response),
        }),
        (BrokerRequest::IntelligenceContext(_), response) => {
            Err(BrokerHostError::UnexpectedResponse {
                expected: "correlated knowledge_context",
                actual: response_name(response),
            })
        }
        (BrokerRequest::OutcomeFeedback(_), response) => Err(BrokerHostError::UnexpectedResponse {
            expected: "correlated outcome_feedback_accepted",
            actual: response_name(response),
        }),
        (BrokerRequest::KnowledgeCollection(_), response) => {
            Err(BrokerHostError::UnexpectedResponse {
                expected: "knowledge_collection",
                actual: response_name(response),
            })
        }
        _ => Err(BrokerHostError::UnexpectedResponse {
            expected: "known broker response",
            actual: response_name(response),
        }),
    }
}

fn response_name(response: &BrokerResponse) -> &'static str {
    match response {
        BrokerResponse::Probe(_) => "probe",
        BrokerResponse::DatabaseRows(_) => "database_rows",
        BrokerResponse::DatabaseMutation(_) => "database_mutation",
        BrokerResponse::SshExecution(_) => "ssh_execution",
        BrokerResponse::KnowledgeContext(_) => "knowledge_context",
        BrokerResponse::OutcomeFeedbackAccepted(_) => "outcome_feedback_accepted",
        BrokerResponse::KnowledgeCollection(_) => "knowledge_collection",
        BrokerResponse::Error(_) => "error",
        _ => "unknown",
    }
}

async fn read_frame<R>(
    reader: &mut BufReader<R>,
    maximum: usize,
) -> Result<Option<Vec<u8>>, BrokerHostError>
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
        return Err(BrokerHostError::FrameTooLarge {
            actual: frame.len(),
            maximum,
        });
    }
    if frame.pop() != Some(b'\n') {
        return Err(BrokerHostError::UnterminatedFrame);
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Ok(Some(frame))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use gadgetron_bundle_sdk::{
        BrokerProbeRequest, BrokerProbeResult, BrokerResource, BundleId, BundlePackageManifest,
        DatabaseInsertRequest, DatabaseMutationResult, DatabaseRows, DatabaseSelectRequest,
        InvocationLeaseToken, SshExecuteRequest,
    };
    use semver::Version;
    use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt};

    const PACKAGE: &str = r#"
manifest_version = 1

[bundle]
id = "server-administrator"
version = "0.1.0"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/server-administrator"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[capabilities]
gadget_namespaces = ["server"]
"#;

    #[derive(Debug)]
    struct FixtureBroker;

    #[async_trait]
    impl BundleBroker for FixtureBroker {
        async fn handle(&self, caller: &BrokerCaller, request: BrokerRequest) -> BrokerResponse {
            assert_eq!(caller.identity().id.as_str(), "server-administrator");
            assert_eq!(caller.package_manifest_sha256().len(), 64);
            match request {
                BrokerRequest::Probe(request) => BrokerResponse::Probe(BrokerProbeResult::ready(
                    request.permission_id,
                    request.resource,
                )),
                BrokerRequest::DatabaseSelect(_) => {
                    BrokerResponse::DatabaseRows(DatabaseRows::new(Vec::new(), false))
                }
                BrokerRequest::DatabaseInsert(_)
                | BrokerRequest::DatabaseUpdate(_)
                | BrokerRequest::DatabaseDelete(_) => {
                    BrokerResponse::DatabaseMutation(DatabaseMutationResult::new(1))
                }
                BrokerRequest::SshExecute(request) => {
                    BrokerResponse::SshExecution(gadgetron_bundle_sdk::SshExecutionResult::new(
                        request.target_id,
                        request.operation_id,
                        0,
                        String::new(),
                        String::new(),
                        1,
                    ))
                }
                _ => unreachable!("fixture covers all current broker operations"),
            }
        }
    }

    fn package() -> ValidatedPackageContract {
        ValidatedPackageContract::parse(PACKAGE, &Version::new(1, 0, 0)).unwrap()
    }

    async fn exchange(
        io: &mut tokio::io::DuplexStream,
        request: &BrokerEnvelope<BrokerRequest>,
    ) -> BrokerEnvelope<BrokerResponse> {
        let mut encoded = serde_json::to_vec(request).unwrap();
        encoded.push(b'\n');
        io.write_all(&encoded).await.unwrap();
        io.flush().await.unwrap();
        let mut line = Vec::new();
        BufReader::new(&mut *io)
            .read_until(b'\n', &mut line)
            .await
            .unwrap();
        serde_json::from_slice(&line).unwrap()
    }

    #[tokio::test]
    async fn probe_and_database_requests_are_identity_and_digest_bound() {
        let package = package();
        let caller = BrokerCaller::from_package(&package);
        let identity = caller.identity().clone();
        let (mut client, server) = duplex(64 * 1024);
        let task = tokio::spawn(serve_broker_channel(
            server,
            caller,
            Arc::new(FixtureBroker),
            BrokerChannelLimits::default(),
        ));

        let probe = BrokerEnvelope::new(
            "bundle:1",
            identity.clone(),
            BrokerRequest::Probe(BrokerProbeRequest::new(
                LocalId::new("telemetry-read").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
            )),
        );
        let response = exchange(&mut client, &probe).await;
        assert_eq!(response.message_id, "bundle:1");
        assert!(matches!(response.payload, BrokerResponse::Probe(_)));

        let select = BrokerEnvelope::new(
            "bundle:2",
            identity.clone(),
            BrokerRequest::DatabaseSelect(DatabaseSelectRequest::new(
                InvocationLeaseToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
                LocalId::new("telemetry-read").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
                ["host_id".to_string()],
            )),
        );
        let response = exchange(&mut client, &select).await;
        assert!(matches!(response.payload, BrokerResponse::DatabaseRows(_)));

        let insert = BrokerEnvelope::new(
            "bundle:3",
            identity.clone(),
            BrokerRequest::DatabaseInsert(DatabaseInsertRequest::new(
                InvocationLeaseToken::new("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB").unwrap(),
                LocalId::new("telemetry-write").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
                BTreeMap::from([(
                    "host_id".into(),
                    serde_json::json!("00000000-0000-0000-0000-000000000000"),
                )]),
            )),
        );
        let response = exchange(&mut client, &insert).await;
        assert!(matches!(
            response.payload,
            BrokerResponse::DatabaseMutation(ref result) if result.affected_rows == 1
        ));

        let ssh = BrokerEnvelope::new(
            "bundle:4",
            identity,
            BrokerRequest::SshExecute(SshExecuteRequest::new(
                InvocationLeaseToken::new("CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC").unwrap(),
                LocalId::new("edge-one").unwrap(),
                LocalId::new("inventory").unwrap(),
            )),
        );
        let response = exchange(&mut client, &ssh).await;
        assert!(matches!(
            response.payload,
            BrokerResponse::SshExecution(ref result)
                if result.target_id.as_str() == "edge-one"
                    && result.operation_id.as_str() == "inventory"
        ));

        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn wrong_identity_and_duplicate_message_ids_fail_the_channel() {
        let package = package();
        let caller = BrokerCaller::from_package(&package);
        let expected_identity = caller.identity().clone();
        let (mut client, server) = duplex(64 * 1024);
        let task = tokio::spawn(serve_broker_channel(
            server,
            caller,
            Arc::new(FixtureBroker),
            BrokerChannelLimits::default(),
        ));
        let wrong = BrokerEnvelope::new(
            "bundle:1",
            BundleRuntimeIdentity::new(
                BundleId::new("restaurant-research").unwrap(),
                Version::new(1, 0, 0),
            ),
            BrokerRequest::Probe(BrokerProbeRequest::new(
                LocalId::new("telemetry-read").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
            )),
        );
        let mut encoded = serde_json::to_vec(&wrong).unwrap();
        encoded.push(b'\n');
        client.write_all(&encoded).await.unwrap();
        assert!(matches!(
            task.await.unwrap(),
            Err(BrokerHostError::Contract(_))
        ));

        let caller = BrokerCaller::from_package(&package);
        let (mut client, server) = duplex(64 * 1024);
        let task = tokio::spawn(serve_broker_channel(
            server,
            caller,
            Arc::new(FixtureBroker),
            BrokerChannelLimits::default(),
        ));
        let request = BrokerEnvelope::new(
            "bundle:repeat",
            expected_identity,
            BrokerRequest::Probe(BrokerProbeRequest::new(
                LocalId::new("telemetry-read").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
            )),
        );
        let _ = exchange(&mut client, &request).await;
        let mut encoded = serde_json::to_vec(&request).unwrap();
        encoded.push(b'\n');
        client.write_all(&encoded).await.unwrap();
        assert!(matches!(
            task.await.unwrap(),
            Err(BrokerHostError::DuplicateMessageId(ref id)) if id == "bundle:repeat"
        ));
    }

    #[tokio::test]
    async fn long_lived_channel_rotates_its_duplicate_detection_window() {
        let package = package();
        let caller = BrokerCaller::from_package(&package);
        let identity = caller.identity().clone();
        let (mut client, server) = duplex(64 * 1024);
        let task = tokio::spawn(serve_broker_channel(
            server,
            caller,
            Arc::new(FixtureBroker),
            BrokerChannelLimits::new(
                DEFAULT_BROKER_REQUEST_TIMEOUT,
                DEFAULT_BROKER_FRAME_BYTES,
                2,
            ),
        ));

        for counter in 1..=3 {
            let request = BrokerEnvelope::new(
                format!("bundle:{counter}"),
                identity.clone(),
                BrokerRequest::Probe(BrokerProbeRequest::new(
                    LocalId::new("telemetry-read").unwrap(),
                    BrokerResource::database_table("host_stats_latest").unwrap(),
                )),
            );
            let response = exchange(&mut client, &request).await;
            assert!(matches!(response.payload, BrokerResponse::Probe(_)));
        }

        let duplicate = BrokerEnvelope::new(
            "bundle:3",
            identity,
            BrokerRequest::Probe(BrokerProbeRequest::new(
                LocalId::new("telemetry-read").unwrap(),
                BrokerResource::database_table("host_stats_latest").unwrap(),
            )),
        );
        let mut encoded = serde_json::to_vec(&duplicate).unwrap();
        encoded.push(b'\n');
        client.write_all(&encoded).await.unwrap();
        assert!(matches!(
            task.await.unwrap(),
            Err(BrokerHostError::DuplicateMessageId(ref id)) if id == "bundle:3"
        ));
    }

    #[test]
    fn public_contract_remains_sdk_only() {
        let manifest = BundlePackageManifest::parse_toml(PACKAGE).unwrap();
        assert_eq!(manifest.bundle.id.as_str(), "server-administrator");
    }
}
