use std::time::Duration;

use gadgetron_bundle_sdk::{
    Acknowledgement, BundleRuntimeIdentity, GadgetInvocation, GadgetResult, HandshakeRequest,
    HealthReport, HealthRequest, HostError, HostRequest, HostResponse, JobAccepted,
    JobCancelRequest, JobPollRequest, JobStartRequest, JobStatusReport, ProtocolEnvelope,
    ShutdownRequest, BUNDLE_HOST_PROTOCOL_VERSION,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    time::{timeout, Instant},
};

use crate::{BundleHostError, Result, ValidatedPackageContract};

pub const DEFAULT_FRAME_BYTES: usize = 1_048_576;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_STALE_RESPONSE_FRAMES: usize = 64;

/// Correlated request/response session over a supervisor-provided channel.
pub struct BundleHostSession<R, W> {
    reader: BufReader<R>,
    writer: W,
    identity: BundleRuntimeIdentity,
    manifest_sha256: String,
    next_message_id: u64,
    request_timeout: Duration,
    max_frame_bytes: usize,
    handshaken: bool,
}

impl<R, W> BundleHostSession<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub fn attach(reader: R, writer: W, package: &ValidatedPackageContract) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            identity: package.runtime_identity(),
            manifest_sha256: package.manifest_sha256().to_string(),
            next_message_id: 1,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_frame_bytes: DEFAULT_FRAME_BYTES,
            handshaken: false,
        }
    }

    pub fn with_limits(mut self, request_timeout: Duration, max_frame_bytes: usize) -> Self {
        self.request_timeout = request_timeout;
        self.max_frame_bytes = max_frame_bytes.max(1);
        self
    }

    pub async fn handshake(&mut self) -> Result<()> {
        let response = self
            .exchange(HostRequest::Handshake(HandshakeRequest::new(
                self.manifest_sha256.clone(),
                BUNDLE_HOST_PROTOCOL_VERSION,
                BUNDLE_HOST_PROTOCOL_VERSION,
            )))
            .await?;
        let response = match response {
            HostResponse::Handshake(response) => response,
            HostResponse::Error(error) => return Err(remote_error(error)),
            response => {
                return Err(BundleHostError::UnexpectedResponse {
                    expected: "handshake",
                    actual: response_name(&response),
                })
            }
        };
        if response.package_manifest_sha256 != self.manifest_sha256 {
            return Err(BundleHostError::ManifestDigestMismatch {
                expected: self.manifest_sha256.clone(),
                actual: response.package_manifest_sha256,
            });
        }
        if response.selected_protocol != BUNDLE_HOST_PROTOCOL_VERSION {
            return Err(BundleHostError::UnexpectedResponse {
                expected: "host protocol v1 handshake",
                actual: "unsupported protocol selection",
            });
        }
        self.handshaken = true;
        Ok(())
    }

    pub async fn health(&mut self) -> Result<HealthReport> {
        self.require_handshake()?;
        let response = self
            .exchange(HostRequest::Health(HealthRequest::default()))
            .await?;
        let report = match response {
            HostResponse::Health(report) => report,
            HostResponse::Error(error) => return Err(remote_error(error)),
            response => {
                return Err(BundleHostError::UnexpectedResponse {
                    expected: "health",
                    actual: response_name(&response),
                })
            }
        };
        Ok(report)
    }

    pub async fn invoke_gadget(&mut self, invocation: GadgetInvocation) -> Result<GadgetResult> {
        self.require_handshake()?;
        let response = self.exchange(HostRequest::InvokeGadget(invocation)).await?;
        let result = match response {
            HostResponse::GadgetResult(result) => result,
            HostResponse::Error(error) => return Err(remote_error(error)),
            response => {
                return Err(BundleHostError::UnexpectedResponse {
                    expected: "gadget_result",
                    actual: response_name(&response),
                })
            }
        };
        Ok(result)
    }

    pub async fn start_job(&mut self, request: JobStartRequest) -> Result<JobAccepted> {
        self.require_handshake()?;
        match self.exchange(HostRequest::StartJob(request)).await? {
            HostResponse::JobAccepted(job) => Ok(job),
            HostResponse::Error(error) => Err(remote_error(error)),
            response => Err(BundleHostError::UnexpectedResponse {
                expected: "job_accepted",
                actual: response_name(&response),
            }),
        }
    }

    pub async fn poll_job(&mut self, request: JobPollRequest) -> Result<JobStatusReport> {
        self.require_handshake()?;
        match self.exchange(HostRequest::PollJob(request)).await? {
            HostResponse::JobStatus(job) => Ok(job),
            HostResponse::Error(error) => Err(remote_error(error)),
            response => Err(BundleHostError::UnexpectedResponse {
                expected: "job_status",
                actual: response_name(&response),
            }),
        }
    }

    pub async fn cancel_job(&mut self, request: JobCancelRequest) -> Result<JobStatusReport> {
        self.require_handshake()?;
        match self.exchange(HostRequest::CancelJob(request)).await? {
            HostResponse::JobStatus(job) => Ok(job),
            HostResponse::Error(error) => Err(remote_error(error)),
            response => Err(BundleHostError::UnexpectedResponse {
                expected: "job_status",
                actual: response_name(&response),
            }),
        }
    }

    pub async fn shutdown(&mut self, request: ShutdownRequest) -> Result<Acknowledgement> {
        self.require_handshake()?;
        let response = self.exchange(HostRequest::Shutdown(request)).await?;
        let acknowledgement = match response {
            HostResponse::Acknowledgement(acknowledgement) => acknowledgement,
            HostResponse::Error(error) => return Err(remote_error(error)),
            response => {
                return Err(BundleHostError::UnexpectedResponse {
                    expected: "acknowledgement",
                    actual: response_name(&response),
                })
            }
        };
        Ok(acknowledgement)
    }

    fn require_handshake(&self) -> Result<()> {
        if self.handshaken {
            Ok(())
        } else {
            Err(BundleHostError::HandshakeRequired)
        }
    }

    async fn exchange(&mut self, payload: HostRequest) -> Result<HostResponse> {
        payload.validate()?;
        let expected_sequence = self.next_message_id;
        let message_id = format!("core:{expected_sequence}");
        self.next_message_id = self.next_message_id.saturating_add(1);
        let envelope = ProtocolEnvelope::new(message_id.clone(), self.identity.clone(), payload);
        envelope.validate_routing(&self.identity, BUNDLE_HOST_PROTOCOL_VERSION)?;

        let mut encoded = serde_json::to_vec(&envelope)?;
        if encoded.len() > self.max_frame_bytes {
            return Err(BundleHostError::FrameTooLarge {
                actual: encoded.len(),
                maximum: self.max_frame_bytes,
            });
        }
        encoded.push(b'\n');
        self.writer.write_all(&encoded).await?;
        self.writer.flush().await?;

        let deadline = Instant::now() + self.request_timeout;
        for stale_count in 0..=MAX_STALE_RESPONSE_FRAMES {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(BundleHostError::Timeout(self.request_timeout));
            }
            let frame = timeout(remaining, self.read_frame())
                .await
                .map_err(|_| BundleHostError::Timeout(self.request_timeout))??;
            let response: ProtocolEnvelope<HostResponse> = serde_json::from_slice(&frame)?;
            response.validate_routing(&self.identity, BUNDLE_HOST_PROTOCOL_VERSION)?;
            response.payload.validate()?;
            if response.message_id == message_id {
                return Ok(response.payload);
            }
            let stale = host_response_sequence(&response.message_id)
                .is_some_and(|sequence| sequence < expected_sequence);
            if stale && stale_count < MAX_STALE_RESPONSE_FRAMES {
                continue;
            }
            return Err(BundleHostError::MessageIdMismatch {
                expected: message_id,
                actual: response.message_id,
            });
        }
        unreachable!("bounded stale response loop returns on every terminal branch")
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>> {
        let mut frame = Vec::new();
        let mut limited = (&mut self.reader).take((self.max_frame_bytes + 1) as u64);
        let read = limited.read_until(b'\n', &mut frame).await?;
        if read == 0 {
            return Err(BundleHostError::EndOfStream);
        }
        if frame.len() > self.max_frame_bytes {
            return Err(BundleHostError::FrameTooLarge {
                actual: frame.len(),
                maximum: self.max_frame_bytes,
            });
        }
        if frame.pop() != Some(b'\n') {
            return Err(BundleHostError::UnterminatedFrame);
        }
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
        Ok(frame)
    }
}

fn host_response_sequence(message_id: &str) -> Option<u64> {
    let sequence = message_id.strip_prefix("core:")?.parse::<u64>().ok()?;
    (message_id == format!("core:{sequence}")).then_some(sequence)
}

fn remote_error(error: HostError) -> BundleHostError {
    BundleHostError::Remote {
        code: error.code.to_string(),
        message: error.message,
        retryable: error.retryable,
        details: error.details,
    }
}

fn response_name(response: &HostResponse) -> &'static str {
    match response {
        HostResponse::Handshake(_) => "handshake",
        HostResponse::Health(_) => "health",
        HostResponse::GadgetResult(_) => "gadget_result",
        HostResponse::JobAccepted(_) => "job_accepted",
        HostResponse::JobStatus(_) => "job_status",
        HostResponse::Acknowledgement(_) => "acknowledgement",
        HostResponse::Error(_) => "error",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_bundle_sdk::{
        BundlePackageManifest, GadgetName, HandshakeResponse, HealthStatus, InvocationContext,
    };
    use semver::Version;
    use tokio::io::{duplex, split};
    use tokio::sync::oneshot;

    const PACKAGE: &str = r#"
manifest_version = 1

[bundle]
id = "example-research"
version = "1.0.0"
publisher = "example.publisher"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/example-research"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[capabilities]
gadget_namespaces = ["example"]

[[capabilities.gadgets]]
name = "example.inspect"
description = "Inspect a bounded example"
tier = "read"
input_schema = { type = "object" }
output_schema = { type = "object" }

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = true
"#;

    #[tokio::test]
    async fn handshake_health_invoke_and_shutdown_round_trip() {
        let package = ValidatedPackageContract::parse(PACKAGE, &Version::new(1, 0, 0)).unwrap();
        let identity = package.runtime_identity();
        let digest = package.manifest_sha256().to_string();
        let (client_io, server_io) = duplex(64 * 1024);
        let (client_read, client_write) = split(client_io);
        let (server_read, server_write) = split(server_io);

        let server = tokio::spawn(async move {
            serve_fake_host(server_read, server_write, identity, digest).await;
        });

        let mut session = BundleHostSession::attach(client_read, client_write, &package);
        assert!(matches!(
            session.health().await,
            Err(BundleHostError::HandshakeRequired)
        ));
        session.handshake().await.unwrap();
        assert_eq!(
            session.health().await.unwrap().status,
            HealthStatus::Healthy
        );

        let context = InvocationContext::new("tenant-1", "actor-1", "request-1");
        let result = session
            .invoke_gadget(GadgetInvocation::new(
                GadgetName::new("example.inspect").unwrap(),
                serde_json::json!({"subject": "bounded"}),
                context,
            ))
            .await
            .unwrap();
        assert_eq!(result.output, serde_json::json!({"observed": true}));
        let remote = session
            .invoke_gadget(GadgetInvocation::new(
                GadgetName::new("example.inspect").unwrap(),
                serde_json::json!({"fail": true}),
                InvocationContext::new("tenant-1", "actor-1", "request-2"),
            ))
            .await
            .unwrap_err();
        assert!(matches!(
            remote,
            BundleHostError::Remote {
                ref code,
                retryable: false,
                ..
            } if code == "fixture-failed"
        ));
        let accepted = session
            .start_job(JobStartRequest::new(
                gadgetron_bundle_sdk::LocalId::new("fixture-job").unwrap(),
                serde_json::json!({}),
                InvocationContext::new("tenant-1", "actor-1", "request-job"),
            ))
            .await
            .unwrap();
        assert_eq!(accepted.job_id, "fixture-job-1");
        let status = session
            .poll_job(JobPollRequest::new(&accepted.job_id))
            .await
            .unwrap();
        assert_eq!(status.status, gadgetron_bundle_sdk::JobStatus::Succeeded);
        let cancelled = session
            .cancel_job(JobCancelRequest::new(&accepted.job_id))
            .await
            .unwrap();
        assert_eq!(cancelled.status, gadgetron_bundle_sdk::JobStatus::Cancelled);
        let ack = session.shutdown(ShutdownRequest::default()).await.unwrap();
        assert_eq!(ack.message, "stopping");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn abandoned_response_is_drained_before_the_next_exchange() {
        let package = ValidatedPackageContract::parse(PACKAGE, &Version::new(1, 0, 0)).unwrap();
        let identity = package.runtime_identity();
        let digest = package.manifest_sha256().to_string();
        let (client_io, server_io) = duplex(64 * 1024);
        let (client_read, client_write) = split(client_io);
        let (server_read, mut server_write) = split(server_io);
        let (stale_seen_tx, stale_seen_rx) = oneshot::channel();
        let (release_stale_tx, release_stale_rx) = oneshot::channel();

        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let handshake: ProtocolEnvelope<HostRequest> = serde_json::from_slice(&line).unwrap();
            let response = ProtocolEnvelope::new(
                handshake.message_id,
                identity.clone(),
                HostResponse::Handshake(HandshakeResponse::new(
                    digest,
                    BUNDLE_HOST_PROTOCOL_VERSION,
                )),
            );
            let mut encoded = serde_json::to_vec(&response).unwrap();
            encoded.push(b'\n');
            server_write.write_all(&encoded).await.unwrap();
            server_write.flush().await.unwrap();

            line.clear();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let abandoned: ProtocolEnvelope<HostRequest> = serde_json::from_slice(&line).unwrap();
            stale_seen_tx.send(()).unwrap();
            release_stale_rx.await.unwrap();
            let stale_response = ProtocolEnvelope::new(
                abandoned.message_id,
                identity.clone(),
                HostResponse::Health(HealthReport::healthy()),
            );
            let mut encoded = serde_json::to_vec(&stale_response).unwrap();
            encoded.push(b'\n');
            server_write.write_all(&encoded).await.unwrap();
            server_write.flush().await.unwrap();

            line.clear();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let current: ProtocolEnvelope<HostRequest> = serde_json::from_slice(&line).unwrap();
            let current_response = ProtocolEnvelope::new(
                current.message_id,
                identity,
                HostResponse::Health(HealthReport::healthy()),
            );
            let mut encoded = serde_json::to_vec(&current_response).unwrap();
            encoded.push(b'\n');
            server_write.write_all(&encoded).await.unwrap();
            server_write.flush().await.unwrap();
        });

        let mut session = BundleHostSession::attach(client_read, client_write, &package);
        session.handshake().await.unwrap();
        {
            let mut abandoned = Box::pin(session.health());
            tokio::select! {
                observed = stale_seen_rx => observed.unwrap(),
                result = &mut abandoned => panic!("fixture released abandoned response early: {result:?}"),
            }
        }
        release_stale_tx.send(()).unwrap();

        assert_eq!(
            session.health().await.unwrap().status,
            HealthStatus::Healthy
        );
        server.await.unwrap();
    }

    #[test]
    fn only_canonical_prior_host_response_ids_are_stale() {
        assert_eq!(host_response_sequence("core:29"), Some(29));
        assert_eq!(host_response_sequence("core:029"), None);
        assert_eq!(host_response_sequence("runtime:29"), None);
        assert_eq!(host_response_sequence("core:not-a-number"), None);
    }

    async fn serve_fake_host<R, W>(
        reader: R,
        mut writer: W,
        identity: BundleRuntimeIdentity,
        digest: String,
    ) where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut reader = BufReader::new(reader);
        loop {
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line).await.unwrap() == 0 {
                break;
            }
            let request: ProtocolEnvelope<HostRequest> = serde_json::from_slice(&line).unwrap();
            let should_stop = matches!(request.payload, HostRequest::Shutdown(_));
            let payload = match request.payload {
                HostRequest::Handshake(_) => HostResponse::Handshake(HandshakeResponse::new(
                    digest.clone(),
                    BUNDLE_HOST_PROTOCOL_VERSION,
                )),
                HostRequest::Health(_) => HostResponse::Health(HealthReport::healthy()),
                HostRequest::InvokeGadget(invocation)
                    if invocation
                        .input
                        .get("fail")
                        .and_then(|value| value.as_bool())
                        == Some(true) =>
                {
                    HostResponse::Error(gadgetron_bundle_sdk::HostError::new(
                        gadgetron_bundle_sdk::LocalId::new("fixture-failed").unwrap(),
                        "fixture requested a failure",
                        false,
                    ))
                }
                HostRequest::InvokeGadget(_) => HostResponse::GadgetResult(GadgetResult::new(
                    serde_json::json!({"observed": true}),
                )),
                HostRequest::StartJob(_) => {
                    HostResponse::JobAccepted(JobAccepted::new("fixture-job-1"))
                }
                HostRequest::PollJob(request) => HostResponse::JobStatus(JobStatusReport::new(
                    request.job_id,
                    gadgetron_bundle_sdk::JobStatus::Succeeded,
                )),
                HostRequest::CancelJob(request) => HostResponse::JobStatus(JobStatusReport::new(
                    request.job_id,
                    gadgetron_bundle_sdk::JobStatus::Cancelled,
                )),
                HostRequest::Shutdown(_) => {
                    HostResponse::Acknowledgement(Acknowledgement::new("stopping"))
                }
                _ => HostResponse::Error(gadgetron_bundle_sdk::HostError::new(
                    gadgetron_bundle_sdk::LocalId::new("unsupported").unwrap(),
                    "unsupported request",
                    false,
                )),
            };
            let response = ProtocolEnvelope::new(request.message_id, identity.clone(), payload);
            let mut encoded = serde_json::to_vec(&response).unwrap();
            encoded.push(b'\n');
            writer.write_all(&encoded).await.unwrap();
            writer.flush().await.unwrap();
            if should_stop {
                break;
            }
        }
    }

    #[test]
    fn package_fixture_is_a_valid_sdk_manifest() {
        BundlePackageManifest::parse_toml(PACKAGE).unwrap();
    }
}
