//! Independent Travel Planner Bundle runtime.

use std::sync::Arc;

use gadgetron_bundle_runtime::{BrokerClientError, BundleBrokerClient};
use gadgetron_bundle_sdk::{
    Acknowledgement, BrokerResource, BrokerResourceReadiness, BundleId, BundleRuntimeIdentity,
    GadgetResult, HandshakeResponse, HealthReport, HealthStatus, HostError, HostRequest,
    HostResponse, LocalId, ProtocolEnvelope, BUNDLE_HOST_PROTOCOL_VERSION,
};
use semver::Version;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Mutex,
};

mod travel;

pub const DEFAULT_MAX_FRAME_BYTES: usize = 1_048_576;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid SDK contract: {0}")]
    Sdk(#[from] gadgetron_bundle_sdk::BundleSdkError),
    #[error("manifest SHA-256 must be exactly 64 lowercase hexadecimal characters")]
    InvalidManifestDigest,
    #[error("protocol frame is larger than {maximum} bytes")]
    FrameTooLarge { maximum: usize },
    #[error("protocol frame ended without a newline")]
    UnterminatedFrame,
}

pub type Result<T> = std::result::Result<T, RuntimeError>;
pub(crate) type SharedBroker = Arc<Mutex<BundleBrokerClient>>;

pub struct TravelPlannerRuntime {
    identity: BundleRuntimeIdentity,
    manifest_sha256: String,
    max_frame_bytes: usize,
    handshaken: bool,
    broker: Option<SharedBroker>,
}

impl TravelPlannerRuntime {
    pub fn new(manifest_sha256: impl Into<String>) -> Result<Self> {
        let manifest_sha256 = manifest_sha256.into();
        if manifest_sha256.len() != 64
            || !manifest_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(RuntimeError::InvalidManifestDigest);
        }
        Ok(Self {
            identity: BundleRuntimeIdentity::new(
                BundleId::new("travel-planner")?,
                Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid semver"),
            ),
            manifest_sha256,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            handshaken: false,
            broker: None,
        })
    }

    pub fn identity(&self) -> &BundleRuntimeIdentity {
        &self.identity
    }

    pub fn with_broker(mut self, broker: BundleBrokerClient) -> Self {
        self.broker = Some(Arc::new(Mutex::new(broker)));
        self
    }

    pub async fn serve<R, W>(&mut self, reader: R, mut writer: W) -> Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut reader = BufReader::new(reader);
        loop {
            let Some(frame) = read_frame(&mut reader, self.max_frame_bytes).await? else {
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
            if encoded.len() > self.max_frame_bytes {
                return Err(RuntimeError::FrameTooLarge {
                    maximum: self.max_frame_bytes,
                });
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
                        host_error(
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
                        host_error(
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
                HostResponse::Acknowledgement(Acknowledgement::new("travel-planner stopping")),
                true,
            ),
            _ if !self.handshaken => (
                host_error(
                    "handshake-required",
                    "complete the package-bound handshake before using the runtime",
                ),
                false,
            ),
            HostRequest::Health(_) => (HostResponse::Health(self.health().await), false),
            HostRequest::InvokeGadget(invocation) => {
                let Some(broker) = self.broker.clone() else {
                    return (
                        host_error("broker-unavailable", "Core broker channel is unavailable"),
                        false,
                    );
                };
                (travel::invoke(invocation, broker).await, false)
            }
            HostRequest::Shutdown(_) => unreachable!("handshake guard handles shutdown"),
            _ => (
                host_error(
                    "request-not-supported",
                    "Travel Planner supports health and Gadget invocation only",
                ),
                false,
            ),
        }
    }

    async fn health(&self) -> HealthReport {
        let Some(broker) = self.broker.as_ref() else {
            return HealthReport::with_message(
                HealthStatus::Degraded,
                "Core broker channel is unavailable",
            );
        };
        for table in [
            "travel_trips",
            "travel_itinerary_items",
            "travel_constraints",
            "travel_budget_items",
            "travel_disruptions",
            "travel_replans",
            "travel_operation_outcomes",
        ] {
            for permission in ["travel-read", "travel-write"] {
                let resource =
                    BrokerResource::database_table(table).expect("static table resource is valid");
                match broker
                    .lock()
                    .await
                    .probe(
                        LocalId::new(permission).expect("static permission is valid"),
                        resource,
                    )
                    .await
                {
                    Ok(result) if result.readiness == BrokerResourceReadiness::Ready => {}
                    Ok(result) => {
                        return HealthReport::with_message(
                            HealthStatus::Degraded,
                            result
                                .message
                                .unwrap_or_else(|| format!("{table} is unavailable")),
                        )
                    }
                    Err(error) => {
                        return HealthReport::with_message(
                            HealthStatus::Degraded,
                            format!("{table} probe failed: {}", error.public_message()),
                        )
                    }
                }
            }
        }
        HealthReport::with_message(
            HealthStatus::Healthy,
            "Travel Planner data dependencies are ready",
        )
    }
}

pub(crate) fn host_error(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(HostError::new(
        LocalId::new(code).expect("static host error code is canonical"),
        message,
        false,
    ))
}

pub(crate) fn broker_error(error: BrokerClientError) -> HostResponse {
    match error {
        BrokerClientError::Remote(error) => {
            HostResponse::Error(HostError::new(error.code, error.message, error.retryable))
        }
        error => host_error("broker-channel-failed", &error.public_message()),
    }
}

pub(crate) fn gadget_result(value: serde_json::Value) -> HostResponse {
    HostResponse::GadgetResult(GadgetResult::new(value))
}

async fn read_frame<R>(reader: &mut BufReader<R>, maximum: usize) -> Result<Option<Vec<u8>>>
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
        return Err(RuntimeError::FrameTooLarge { maximum });
    }
    if frame.pop() != Some(b'\n') {
        return Err(RuntimeError::UnterminatedFrame);
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Ok(Some(frame))
}
