//! Independently shipped Server Administrator Bundle runtime.
//!
//! The first P0.3 capability is a tenant-forced read-only telemetry workspace.
//! It owns no database pool or credential and can reach data only through the
//! Core broker channel.

use std::{collections::BTreeMap, sync::Arc};

pub use gadgetron_bundle_runtime::{BrokerClientError, BundleBrokerClient};
use gadgetron_bundle_sdk::{
    Acknowledgement, BrokerResource, BrokerResourceReadiness, BundleId, BundleRuntimeIdentity,
    DatabaseOrderDirection, DatabaseSelectRequest, GadgetResult, HandshakeResponse, HealthReport,
    HealthStatus, HostError, HostRequest, HostResponse, JobAccepted, JobCancelRequest,
    JobPollRequest, JobStartRequest, JobStatus, JobStatusReport, LocalId, ProtocolEnvelope,
    SshExecuteRequest, BUNDLE_HOST_PROTOCOL_VERSION,
};
use semver::Version;
use serde::Deserialize;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

mod alerts;
mod cooling;
mod enrollment;
mod enrollment_job;
mod logs;
mod metrics;
mod operational;
mod telemetry;
mod topology;

use telemetry::parse_inventory;

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
    #[error("Bundle broker failed: {0}")]
    Broker(#[from] BrokerClientError),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// SDK-only protocol endpoint. It deliberately owns no Core handle, database
/// pool, credential, filesystem path or network client.
pub struct ServerAdministratorRuntime {
    identity: BundleRuntimeIdentity,
    manifest_sha256: String,
    max_frame_bytes: usize,
    handshaken: bool,
    broker: Option<Arc<Mutex<BundleBrokerClient>>>,
    jobs: Arc<Mutex<BTreeMap<String, RuntimeJobState>>>,
}

struct RuntimeJobState {
    status: JobStatus,
    progress: Option<serde_json::Value>,
    result: Option<GadgetResult>,
    abort: tokio::task::AbortHandle,
    recipe_id: String,
    target_id: String,
    lease: gadgetron_bundle_sdk::InvocationLeaseToken,
    started_at: String,
    actor_ref: String,
}

#[derive(Clone, Copy)]
enum ServerJobRecipe {
    DutyCycle,
    Enrollment,
}

impl ServerJobRecipe {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "server-duty-cycle" => Some(Self::DutyCycle),
            "server-enrollment" => Some(Self::Enrollment),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::DutyCycle => "server-duty-cycle",
            Self::Enrollment => "server-enrollment",
        }
    }

    fn initial_progress(self) -> serde_json::Value {
        match self {
            Self::DutyCycle => {
                serde_json::json!({"stage":"monitoring-observation","completed":0,"total":6})
            }
            Self::Enrollment => {
                serde_json::json!({"stage":"commissioning","completed":0,"total":6})
            }
        }
    }

    fn completed_progress(self) -> serde_json::Value {
        match self {
            Self::DutyCycle => serde_json::json!({"stage":"complete","completed":6,"total":6}),
            Self::Enrollment => serde_json::json!({"stage":"active","completed":6,"total":6}),
        }
    }

    async fn run(
        self,
        parameters: serde_json::Value,
        context: gadgetron_bundle_sdk::InvocationContext,
        broker: operational::SharedBroker,
    ) -> std::result::Result<GadgetResult, HostError> {
        match self {
            Self::DutyCycle => operational::run_duty_cycle(parameters, context, broker).await,
            Self::Enrollment => enrollment_job::run(parameters, context, broker).await,
        }
    }
}

impl ServerAdministratorRuntime {
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
                BundleId::new("server-administrator")?,
                Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid semver"),
            ),
            manifest_sha256,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            handshaken: false,
            broker: None,
            jobs: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    pub fn identity(&self) -> &BundleRuntimeIdentity {
        &self.identity
    }

    pub fn with_max_frame_bytes(mut self, max_frame_bytes: usize) -> Self {
        self.max_frame_bytes = max_frame_bytes.max(1);
        self
    }

    pub fn with_broker(mut self, broker: BundleBrokerClient) -> Self {
        self.broker = Some(Arc::new(Mutex::new(broker)));
        self
    }

    /// Serve newline-delimited SDK envelopes until EOF or an acknowledged
    /// shutdown. The caller owns process isolation and supplies the channel.
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
                            "runtime manifest digest does not match the package selected by Core",
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
            HostRequest::Shutdown(_) if self.handshaken => {
                for job in self.jobs.lock().await.values() {
                    job.abort.abort();
                }
                (
                    HostResponse::Acknowledgement(Acknowledgement::new(
                        "server-administrator stopping",
                    )),
                    true,
                )
            }
            _ if !self.handshaken => (
                host_error(
                    "handshake-required",
                    "complete the package-bound handshake before using the runtime",
                ),
                false,
            ),
            HostRequest::Health(_) => (HostResponse::Health(self.health().await), false),
            HostRequest::Shutdown(_) => unreachable!("handshake guard handles shutdown"),
            HostRequest::InvokeGadget(invocation) => (self.invoke(invocation).await, false),
            HostRequest::StartJob(request) => (self.start_job(request).await, false),
            HostRequest::PollJob(request) => (self.poll_job(request).await, false),
            HostRequest::CancelJob(request) => (self.cancel_job(request).await, false),
            _ => (
                host_error(
                    "request-not-supported",
                    "this host-protocol request is not supported by the migration runtime",
                ),
                false,
            ),
        }
    }

    async fn health(&mut self) -> HealthReport {
        let Some(broker) = self.broker.as_ref() else {
            return HealthReport::with_message(
                HealthStatus::Degraded,
                "Core broker channel is unavailable",
            );
        };
        let probes = [
            (
                LocalId::new("operations-read").expect("static permission id is valid"),
                BrokerResource::database_table("host_stats_latest")
                    .expect("static database resource is valid"),
                "operational read database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("host_metrics")
                    .expect("static database resource is valid"),
                "operational write database resource",
            ),
            (
                LocalId::new("operations-read").expect("static permission id is valid"),
                BrokerResource::database_table("server_metric_history_6h")
                    .expect("static database resource is valid"),
                "metric history database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("log_findings")
                    .expect("static database resource is valid"),
                "log finding database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("server_operation_outcomes")
                    .expect("static database resource is valid"),
                "operation outcome database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("server_assets_latest")
                    .expect("static database resource is valid"),
                "server asset database resource",
            ),
            (
                LocalId::new("operations-read").expect("static permission id is valid"),
                BrokerResource::database_table("server_gadgetini_latest")
                    .expect("static database resource is valid"),
                "Gadgetini current-observation database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("server_gadgetini_observations")
                    .expect("static database resource is valid"),
                "Gadgetini history database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("alert_state")
                    .expect("static database resource is valid"),
                "alert database resource",
            ),
            (
                LocalId::new("operations-write").expect("static permission id is valid"),
                BrokerResource::database_table("server_job_runs")
                    .expect("static database resource is valid"),
                "job database resource",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("inventory")
                    .expect("static SSH operation resource is valid"),
                "SSH inventory executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("telemetry")
                    .expect("static SSH operation resource is valid"),
                "SSH telemetry executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("topology")
                    .expect("static SSH operation resource is valid"),
                "SSH topology executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("gadgetini-telemetry")
                    .expect("static SSH operation resource is valid"),
                "SSH Gadgetini telemetry executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("log-scan")
                    .expect("static SSH operation resource is valid"),
                "SSH log scan executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("log-system-errors")
                    .expect("static SSH operation resource is valid"),
                "SSH system error log executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("log-kernel-warnings")
                    .expect("static SSH operation resource is valid"),
                "SSH kernel warning log executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("log-auth-failures")
                    .expect("static SSH operation resource is valid"),
                "SSH authentication failure log executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("monitoring-state")
                    .expect("static SSH operation resource is valid"),
                "SSH monitoring state executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("monitoring-enable")
                    .expect("static SSH operation resource is valid"),
                "SSH monitoring enable executor",
            ),
            (
                LocalId::new("ssh-operations").expect("static permission id is valid"),
                BrokerResource::ssh_operation("monitoring-disable")
                    .expect("static SSH operation resource is valid"),
                "SSH monitoring disable executor",
            ),
            (
                LocalId::new("ssh-key-use").expect("static permission id is valid"),
                BrokerResource::secret_use("ssh-identity")
                    .expect("static secret resource is valid"),
                "SSH secret provider",
            ),
        ];
        for (permission, resource, dependency) in probes {
            match broker.lock().await.probe(permission, resource).await {
                Ok(result) if result.readiness == BrokerResourceReadiness::Ready => {}
                Ok(result) => {
                    return HealthReport::with_message(
                        HealthStatus::Degraded,
                        result
                            .message
                            .unwrap_or_else(|| format!("{dependency} is unavailable")),
                    )
                }
                Err(error) => {
                    return HealthReport::with_message(
                        HealthStatus::Degraded,
                        format!("{dependency} probe failed: {}", error.public_message()),
                    )
                }
            }
        }
        HealthReport::with_message(
            HealthStatus::Healthy,
            "server telemetry and signed SSH inventory dependencies are ready",
        )
    }

    async fn invoke(&mut self, invocation: gadgetron_bundle_sdk::GadgetInvocation) -> HostResponse {
        if operational::supports(invocation.gadget.as_str()) {
            let Some(broker) = self.broker.clone() else {
                return host_error("broker-unavailable", "Core broker channel is unavailable");
            };
            return operational::invoke(invocation, broker).await;
        }
        if !matches!(
            invocation.gadget.as_str(),
            "server.host-stats-list" | "server.host-inventory"
        ) {
            return host_error(
                "capability-not-migrated",
                "requested Server Administrator capability is not available in this package",
            );
        }
        let Some(lease) = invocation.context.broker_lease else {
            return host_error(
                "broker-lease-required",
                "Core did not attach an invocation-scoped broker lease",
            );
        };
        let Some(broker) = self.broker.as_ref() else {
            return host_error("broker-unavailable", "Core broker channel is unavailable");
        };
        match invocation.gadget.as_str() {
            "server.host-stats-list" => {
                let input = match serde_json::from_value::<HostStatsListInput>(invocation.input) {
                    Ok(input) if (1..=200).contains(&input.limit) => input,
                    _ => {
                        return host_error(
                            "invalid-arguments",
                            "limit must be an integer between 1 and 200",
                        )
                    }
                };
                let request = DatabaseSelectRequest::new(
                    lease,
                    LocalId::new("operations-read").expect("static permission id is valid"),
                    BrokerResource::database_table("host_stats_latest")
                        .expect("static database resource is valid"),
                    [
                        "host_id".to_string(),
                        "stats".to_string(),
                        "fetched_at".to_string(),
                    ],
                )
                .with_order("fetched_at", DatabaseOrderDirection::Descending)
                .with_limit(input.limit);
                match broker.lock().await.database_select(request).await {
                    Ok(rows) => HostResponse::GadgetResult(GadgetResult::new(serde_json::json!({
                        "rows": rows.rows,
                        "count": rows.rows.len(),
                        "truncated": rows.truncated,
                    }))),
                    Err(BrokerClientError::Remote(error)) => HostResponse::Error(HostError::new(
                        error.code,
                        error.message,
                        error.retryable,
                    )),
                    Err(error) => host_error("broker-channel-failed", &error.public_message()),
                }
            }
            "server.host-inventory" => {
                let input = match serde_json::from_value::<HostInventoryInput>(invocation.input) {
                    Ok(input) => input,
                    Err(_) => {
                        return host_error(
                            "invalid-arguments",
                            "target_id must be a canonical lowercase kebab-case id",
                        )
                    }
                };
                let target_id = match LocalId::new(input.target_id) {
                    Ok(target_id) => target_id,
                    Err(_) => {
                        return host_error(
                            "invalid-arguments",
                            "target_id must be a canonical lowercase kebab-case id",
                        )
                    }
                };
                let request = SshExecuteRequest::new(
                    lease,
                    target_id.clone(),
                    LocalId::new("inventory").expect("static operation id is valid"),
                );
                match broker.lock().await.ssh_execute(request).await {
                    Ok(result) if result.exit_code == 0 => match parse_inventory(&result.stdout) {
                        Ok(facts) => {
                            HostResponse::GadgetResult(GadgetResult::new(serde_json::json!({
                                "target_id": target_id,
                                "facts": facts,
                                "duration_ms": result.duration_ms,
                            })))
                        }
                        Err(message) => host_error("inventory-output-invalid", message),
                    },
                    Ok(_) => host_error(
                        "ssh-operation-failed",
                        "signed SSH inventory operation returned a non-zero exit status",
                    ),
                    Err(BrokerClientError::Remote(error)) => HostResponse::Error(HostError::new(
                        error.code,
                        error.message,
                        error.retryable,
                    )),
                    Err(error) => host_error("broker-channel-failed", &error.public_message()),
                }
            }
            _ => host_error(
                "capability-not-migrated",
                "requested Server Administrator capability is not available in this package",
            ),
        }
    }

    async fn start_job(&mut self, request: JobStartRequest) -> HostResponse {
        let Some(recipe) = ServerJobRecipe::parse(request.recipe_id.as_str()) else {
            return host_error("job-recipe-not-found", "signed job recipe is not available");
        };
        let target_id = match request
            .parameters
            .get("target_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| LocalId::new(value).ok())
        {
            Some(target_id) => target_id.to_string(),
            None => {
                return host_error(
                    "invalid-arguments",
                    "Server job requires canonical target_id",
                )
            }
        };
        if matches!(recipe, ServerJobRecipe::Enrollment)
            && request
                .parameters
                .get("enrollment_id")
                .and_then(serde_json::Value::as_str)
                .map_or(true, |value| uuid::Uuid::parse_str(value).is_err())
        {
            return host_error(
                "invalid-arguments",
                "server-enrollment requires an enrollment_id UUID",
            );
        }
        let parameters = request.parameters.clone();
        let Some(lease) = request.context.broker_lease.clone() else {
            return host_error(
                "broker-lease-required",
                "Core did not attach a job-scoped broker lease",
            );
        };
        let Some(broker) = self.broker.clone() else {
            return host_error("broker-unavailable", "Core broker channel is unavailable");
        };
        let recipe_id = recipe.as_str();
        let job_id = format!("{recipe_id}-{}", uuid::Uuid::new_v4());
        let started_at = operational::now();
        let actor_ref = request.context.actor_id.clone();
        let initial_progress = recipe.initial_progress();
        if let Err(response) = operational::reconcile_orphaned_job_runs(
            &broker,
            lease.clone(),
            &target_id,
            &started_at,
        )
        .await
        {
            return response;
        }
        if let Err(response) = operational::record_job_state(
            &broker,
            lease.clone(),
            &job_id,
            recipe_id,
            &target_id,
            &actor_ref,
            "running",
            initial_progress.clone(),
            None,
            &started_at,
            None,
        )
        .await
        {
            return response;
        }
        let jobs = self.jobs.clone();
        let task_job_id = job_id.clone();
        let task_target_id = target_id.clone();
        let task_lease = lease.clone();
        let task_started_at = started_at.clone();
        let task_actor_ref = actor_ref.clone();
        let task_recipe_id = recipe_id.to_string();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = start_rx.await;
            let result = recipe
                .run(parameters, request.context, broker.clone())
                .await;
            let finished_at = operational::now();
            let (status, progress, output) = match result {
                Ok(result) => (JobStatus::Succeeded, recipe.completed_progress(), result),
                Err(error) => (
                    JobStatus::Failed,
                    serde_json::json!({"stage":"failed"}),
                    GadgetResult::new(serde_json::json!({
                        "error": {"code": error.code, "message": error.message, "retryable": error.retryable}
                    })),
                ),
            };
            let persisted = operational::record_job_state(
                &broker,
                task_lease,
                &task_job_id,
                &task_recipe_id,
                &task_target_id,
                &task_actor_ref,
                match status {
                    JobStatus::Succeeded => "succeeded",
                    JobStatus::Failed => "failed",
                    _ => unreachable!("Server job terminal status is fixed"),
                },
                progress.clone(),
                Some(output.output.clone()),
                &task_started_at,
                Some(&finished_at),
            )
            .await;
            let mut jobs = jobs.lock().await;
            let Some(job) = jobs.get_mut(&task_job_id) else {
                return;
            };
            if job.status == JobStatus::Cancelled {
                return;
            }
            if let Err(response) = persisted {
                job.status = JobStatus::Failed;
                job.progress = Some(serde_json::json!({"stage":"persistence-failed"}));
                job.result = Some(GadgetResult::new(serde_json::json!({
                    "error": response
                })));
                return;
            }
            job.status = status;
            job.progress = Some(progress);
            job.result = Some(output);
        });
        let abort = task.abort_handle();
        self.jobs.lock().await.insert(
            job_id.clone(),
            RuntimeJobState {
                status: JobStatus::Running,
                progress: Some(initial_progress),
                result: None,
                abort,
                recipe_id: recipe_id.to_string(),
                target_id,
                lease,
                started_at,
                actor_ref,
            },
        );
        let _ = start_tx.send(());
        HostResponse::JobAccepted(JobAccepted::new(job_id))
    }

    async fn poll_job(&self, request: JobPollRequest) -> HostResponse {
        let jobs = self.jobs.lock().await;
        let Some(job) = jobs.get(&request.job_id) else {
            return host_error("job-not-found", "Server Administrator job does not exist");
        };
        let mut report = JobStatusReport::new(request.job_id, job.status);
        report.progress = job.progress.clone();
        report.result = job.result.clone();
        HostResponse::JobStatus(report)
    }

    async fn cancel_job(&self, request: JobCancelRequest) -> HostResponse {
        let mut jobs = self.jobs.lock().await;
        let Some(job) = jobs.get_mut(&request.job_id) else {
            return host_error("job-not-found", "Server Administrator job does not exist");
        };
        let mut cancellation = None;
        if matches!(job.status, JobStatus::Queued | JobStatus::Running) {
            job.abort.abort();
            job.status = JobStatus::Cancelled;
            let reason = request
                .reason
                .unwrap_or_else(|| "cancelled by Manager".into());
            job.progress = Some(serde_json::json!({
                "stage":"cancelled",
                "reason": reason
            }));
            cancellation = Some((
                job.lease.clone(),
                job.recipe_id.clone(),
                job.target_id.clone(),
                job.started_at.clone(),
                job.actor_ref.clone(),
                job.progress.clone().expect("cancelled progress is present"),
            ));
        }
        let mut report = JobStatusReport::new(request.job_id, job.status);
        report.progress = job.progress.clone();
        report.result = job.result.clone();
        drop(jobs);
        if let Some((lease, recipe_id, target_id, started_at, actor_ref, progress)) = cancellation {
            let Some(broker) = self.broker.as_ref() else {
                return host_error("broker-unavailable", "Core broker channel is unavailable");
            };
            let finished_at = operational::now();
            if let Err(response) = operational::record_job_state(
                broker,
                lease,
                &report.job_id,
                &recipe_id,
                &target_id,
                &actor_ref,
                "cancelled",
                progress,
                None,
                &started_at,
                Some(&finished_at),
            )
            .await
            {
                return response;
            }
        }
        HostResponse::JobStatus(report)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostStatsListInput {
    #[serde(default = "default_host_stats_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostInventoryInput {
    target_id: String,
}

fn default_host_stats_limit() -> u32 {
    100
}

pub(crate) fn host_error(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(HostError::new(
        LocalId::new(code).expect("static host error code is canonical"),
        message,
        false,
    ))
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex as StdMutex,
        },
        time::Duration,
    };

    use super::*;
    use gadgetron_bundle_sdk::{
        BrokerEnvelope, BrokerProbeResult, BrokerRequest, BrokerResponse, DatabaseMutationResult,
        DatabaseRows, GadgetInvocation, GadgetName, HandshakeRequest, HealthRequest,
        InvocationContext, InvocationLeaseToken, ObservedOutcome, OutcomeFeedbackReceipt,
        ShutdownRequest, SshExecutionResult,
    };
    use tokio::io::{duplex, split, AsyncBufReadExt, AsyncWriteExt, BufReader};

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn handshake_health_denied_invoke_and_shutdown_are_fail_closed() {
        let runtime = ServerAdministratorRuntime::new(DIGEST).unwrap();
        let identity = runtime.identity().clone();
        let (core_io, runtime_io) = duplex(64 * 1024);
        let (core_read, mut core_write) = split(core_io);
        let (runtime_read, runtime_write) = split(runtime_io);

        let server = tokio::spawn(async move {
            let mut runtime = runtime;
            runtime.serve(runtime_read, runtime_write).await.unwrap();
        });
        let mut core_read = BufReader::new(core_read);

        let before_handshake = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "test:1",
                identity.clone(),
                HostRequest::Health(HealthRequest::default()),
            ),
        )
        .await;
        assert!(
            matches!(before_handshake.payload, HostResponse::Error(ref error) if error.code.as_str() == "handshake-required")
        );

        let handshake = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "test:2",
                identity.clone(),
                HostRequest::Handshake(HandshakeRequest::new(
                    DIGEST,
                    BUNDLE_HOST_PROTOCOL_VERSION,
                    BUNDLE_HOST_PROTOCOL_VERSION,
                )),
            ),
        )
        .await;
        assert!(matches!(handshake.payload, HostResponse::Handshake(_)));

        let health = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "test:3",
                identity.clone(),
                HostRequest::Health(HealthRequest::default()),
            ),
        )
        .await;
        assert!(
            matches!(health.payload, HostResponse::Health(ref report) if report.status == HealthStatus::Degraded)
        );

        let invocation = GadgetInvocation::new(
            GadgetName::new("server.list").unwrap(),
            serde_json::json!({}),
            InvocationContext::new("tenant-1", "actor-1", "request-1"),
        );
        let denied = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "test:4",
                identity.clone(),
                HostRequest::InvokeGadget(invocation),
            ),
        )
        .await;
        assert!(
            matches!(denied.payload, HostResponse::Error(ref error) if error.code.as_str() == "capability-not-migrated")
        );

        let shutdown = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "test:5",
                identity,
                HostRequest::Shutdown(ShutdownRequest::default()),
            ),
        )
        .await;
        assert!(matches!(shutdown.payload, HostResponse::Acknowledgement(_)));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn healthy_runtime_uses_only_the_broker_contract_for_operational_flows() {
        let runtime = ServerAdministratorRuntime::new(DIGEST).unwrap();
        let identity = runtime.identity().clone();
        let (core_io, runtime_io) = duplex(64 * 1024);
        let (core_read, mut core_write) = split(core_io);
        let (runtime_read, runtime_write) = split(runtime_io);
        let (broker_client_io, broker_core_io) = duplex(64 * 1024);
        let broker_client = BundleBrokerClient::attach(broker_client_io, identity.clone());
        let cooling_writes = Arc::new(AtomicUsize::new(0));
        let cooling_relation = Arc::new(StdMutex::new(None::<BTreeMap<String, serde_json::Value>>));
        let monitoring_enabled = Arc::new(AtomicBool::new(false));
        let monitoring_repair_succeeds = Arc::new(AtomicBool::new(true));
        let monitoring_alert_active = Arc::new(AtomicBool::new(false));
        let outcome_feedback_requests = Arc::new(AtomicUsize::new(0));
        let monitoring_incident_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let edge_host_id =
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, b"server-administrator:edge-one")
                .to_string();

        let server = tokio::spawn(async move {
            let mut runtime = runtime.with_broker(broker_client);
            runtime.serve(runtime_read, runtime_write).await.unwrap();
        });
        let broker_identity = identity.clone();
        let observed_cooling_writes = Arc::clone(&cooling_writes);
        let observed_cooling_relation = Arc::clone(&cooling_relation);
        let observed_monitoring_enabled = Arc::clone(&monitoring_enabled);
        let observed_monitoring_repair_succeeds = Arc::clone(&monitoring_repair_succeeds);
        let observed_monitoring_alert = Arc::clone(&monitoring_alert_active);
        let observed_outcome_feedback_requests = Arc::clone(&outcome_feedback_requests);
        let broker = tokio::spawn(async move {
            let (broker_read, mut broker_write) = split(broker_core_io);
            let mut broker_read = BufReader::new(broker_read);
            loop {
                let mut line = Vec::new();
                if broker_read.read_until(b'\n', &mut line).await.unwrap() == 0 {
                    break;
                }
                let request: BrokerEnvelope<BrokerRequest> = serde_json::from_slice(&line).unwrap();
                request.validate_routing(&broker_identity).unwrap();
                request.payload.validate().unwrap();
                assert!(!serde_json::to_string(&request)
                    .unwrap()
                    .contains("tenant_id"));
                let payload = match request.payload {
                    BrokerRequest::Probe(request) => BrokerResponse::Probe(
                        BrokerProbeResult::ready(request.permission_id, request.resource),
                    ),
                    BrokerRequest::DatabaseSelect(request)
                        if request.resource.as_str() == "postgres:table:host_stats_latest" =>
                    {
                        assert_eq!(request.permission_id.as_str(), "operations-read");
                        assert_eq!(request.limit, 1);
                        let mut row = BTreeMap::new();
                        row.insert(
                            "host_id".into(),
                            serde_json::Value::String(
                                "11111111-1111-1111-1111-111111111111".into(),
                            ),
                        );
                        row.insert("stats".into(), serde_json::json!({"cpu_percent": 12.5}));
                        row.insert(
                            "fetched_at".into(),
                            serde_json::Value::String("2026-07-11T12:00:00Z".into()),
                        );
                        BrokerResponse::DatabaseRows(DatabaseRows::new(vec![row], false))
                    }
                    BrokerRequest::DatabaseSelect(request)
                        if request.resource.as_str() == "postgres:table:server_target_health"
                            && request.filters.get("target_id")
                                == Some(&serde_json::json!("edge-one")) =>
                    {
                        BrokerResponse::DatabaseRows(DatabaseRows::new(
                            vec![BTreeMap::from([
                                ("target_id".into(), serde_json::json!("edge-one")),
                                ("host_id".into(), serde_json::json!(edge_host_id)),
                            ])],
                            false,
                        ))
                    }
                    BrokerRequest::DatabaseSelect(request)
                        if request.resource.as_str()
                            == "postgres:table:server_incident_signals" =>
                    {
                        let visible = observed_monitoring_alert.load(Ordering::SeqCst)
                            && request
                                .filters
                                .iter()
                                .all(|(field, value)| match field.as_str() {
                                    "fingerprint" => {
                                        value == &serde_json::json!("monitoring-disabled:edge-one")
                                    }
                                    "incident_id" => {
                                        value == &serde_json::json!(monitoring_incident_id)
                                    }
                                    "rule_key" => {
                                        value == &serde_json::json!("monitoring_disabled")
                                    }
                                    "ended_at" => value.is_null(),
                                    _ => true,
                                });
                        let rows = visible.then(|| {
                            BTreeMap::from([
                                (
                                    "signal_id".into(),
                                    serde_json::json!("bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"),
                                ),
                                (
                                    "incident_id".into(),
                                    serde_json::json!(monitoring_incident_id),
                                ),
                                ("rule_key".into(), serde_json::json!("monitoring_disabled")),
                            ])
                        });
                        BrokerResponse::DatabaseRows(DatabaseRows::new(
                            rows.into_iter().collect(),
                            false,
                        ))
                    }
                    BrokerRequest::DatabaseSelect(request)
                        if request.resource.as_str() == "postgres:table:server_incidents"
                            && request.filters.get("incident_id")
                                == Some(&serde_json::json!(monitoring_incident_id)) =>
                    {
                        BrokerResponse::DatabaseRows(DatabaseRows::new(
                            vec![BTreeMap::from([
                                (
                                    "incident_id".into(),
                                    serde_json::json!(monitoring_incident_id),
                                ),
                                ("host_id".into(), serde_json::json!(edge_host_id)),
                                ("status".into(), serde_json::json!("active")),
                            ])],
                            false,
                        ))
                    }
                    BrokerRequest::DatabaseSelect(request)
                        if request.resource.as_str()
                            == "postgres:table:server_gadgetini_latest" =>
                    {
                        let relation = observed_cooling_relation
                            .lock()
                            .expect("cooling relation fixture lock poisoned")
                            .clone()
                            .filter(|row| {
                                request
                                    .filters
                                    .iter()
                                    .all(|(field, expected)| row.get(field) == Some(expected))
                            });
                        BrokerResponse::DatabaseRows(DatabaseRows::new(
                            relation.into_iter().collect(),
                            false,
                        ))
                    }
                    BrokerRequest::DatabaseSelect(_) => {
                        BrokerResponse::DatabaseRows(DatabaseRows::new(Vec::new(), false))
                    }
                    BrokerRequest::DatabaseInsert(request) => {
                        match request.resource.as_str() {
                            "postgres:table:server_gadgetini_latest" => {
                                assert_eq!(request.permission_id.as_str(), "operations-write");
                                assert_eq!(request.values["gadgetini_id"], "gadgetini-one");
                                assert_eq!(request.values["parent_target_id"], "edge-one");
                                assert_eq!(request.values["coolant_leak_detected"], true);
                                assert_eq!(request.conflict_keys, ["gadgetini_id"]);
                                let mut relation = request.values.clone();
                                relation.insert(
                                    "relation_revision".into(),
                                    serde_json::json!("11111111-2222-4333-8444-555555555555"),
                                );
                                *observed_cooling_relation
                                    .lock()
                                    .expect("cooling relation fixture lock poisoned") =
                                    Some(relation);
                                observed_cooling_writes.fetch_or(1, Ordering::SeqCst);
                            }
                            "postgres:table:server_gadgetini_observations" => {
                                assert!(request.conflict_keys.is_empty());
                                assert_eq!(request.values["observation_status"], "observed");
                                observed_cooling_writes.fetch_or(2, Ordering::SeqCst);
                            }
                            "postgres:table:alert_state"
                                if request.values["fingerprint"]
                                    == "gadgetini:gadgetini-one:leak" =>
                            {
                                assert_eq!(request.values["severity"], "critical");
                                observed_cooling_writes.fetch_or(4, Ordering::SeqCst);
                            }
                            "postgres:table:alert_state"
                                if request.values["fingerprint"]
                                    == "monitoring-disabled:edge-one" =>
                            {
                                assert_eq!(request.values["rule_key"], "monitoring_disabled");
                                assert_eq!(request.values["incident_scope"], "observability");
                                observed_monitoring_alert.store(true, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                        BrokerResponse::DatabaseMutation(DatabaseMutationResult::new(1))
                    }
                    BrokerRequest::DatabaseDelete(request)
                        if request.resource.as_str()
                            == "postgres:table:server_gadgetini_latest" =>
                    {
                        let mut relation = observed_cooling_relation
                            .lock()
                            .expect("cooling relation fixture lock poisoned");
                        let matches = relation.as_ref().is_some_and(|row| {
                            request
                                .filters
                                .iter()
                                .all(|(field, expected)| row.get(field) == Some(expected))
                        });
                        if matches {
                            *relation = None;
                        }
                        BrokerResponse::DatabaseMutation(DatabaseMutationResult::new(u32::from(
                            matches,
                        )))
                    }
                    BrokerRequest::DatabaseDelete(request)
                        if request.resource.as_str() == "postgres:table:alert_state"
                            && request.filters.get("fingerprint")
                                == Some(&serde_json::json!("monitoring-disabled:edge-one")) =>
                    {
                        observed_monitoring_alert.store(false, Ordering::SeqCst);
                        BrokerResponse::DatabaseMutation(DatabaseMutationResult::new(1))
                    }
                    BrokerRequest::DatabaseUpdate(_) | BrokerRequest::DatabaseDelete(_) => {
                        BrokerResponse::DatabaseMutation(DatabaseMutationResult::new(1))
                    }
                    BrokerRequest::SshExecute(request) => {
                        let stdout = match request.operation_id.as_str() {
                            "gadgetini-telemetry" => {
                                assert_eq!(request.target_id.as_str(), "gadgetini-one");
                                "21\n34\n1\n0.8\n1\n1\n39.2\n40.9\n40\n40.8\n0\n"
                            }
                            "inventory" => concat!(
                                "hostname=edge-one\n",
                                "kernel=Linux 6.8\n",
                                "architecture=x86_64\n",
                                "machine_id=machine-one\n",
                                "boot_id=boot-one\n",
                                "cpu_model=Fixture CPU\n",
                                "logical_cpus=16\n",
                                "memory_kib=32768000\n",
                                "os_pretty_name=Fixture Linux\n",
                                "uptime_seconds=10\n",
                                "dmi_uuid=dmi-one\n",
                                "dmi_serial=serial-one\n",
                                "gpu=0,GPU-fixture,Fixture GPU\n"
                            ),
                            "telemetry" => concat!(
                                "===HOST===\nedge-one\n",
                                "===UPTIME===\n10\n",
                                "===LOAD===\n0.25 0.50 0.75 1/10 1\n",
                                "===CPUINFO===\n8\n",
                                "===STAT0===\ncpu 100 0 100 800 0 0 0 0\n",
                                "===NET0===\nInter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n eth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n",
                                "===STAT1===\ncpu 150 0 150 900 0 0 0 0\n",
                                "===NET1===\nInter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n eth0: 1200 0 0 0 0 0 0 0 2600 0 0 0 0 0 0 0\n",
                                "===MEM===\nMemTotal: 1000 kB\nMemAvailable: 400 kB\n",
                                "===DF===\n/dev/root|ext4|1000|500|50%|/\n",
                                "===SENSORS===\n{}\n",
                                "===NVSMI===\n",
                                "===NVSMI_HEALTH===\n",
                                "===DCGM===\n",
                                "===IPMI===\n",
                                "===XID===\n",
                                "===AVAILABILITY===\nsensors=0\nnvidia_smi=0\ndcgm=0\nipmitool=0\n",
                                "===END===\n"
                            ),
                            "topology" => concat!(
                                "===LINK===\n[{\"ifname\":\"eth0\",\"link_type\":\"ether\",\"operstate\":\"UP\"}]\n",
                                "===ADDR===\n[]\n",
                                "===NEIGH===\n[]\n",
                                "===ROUTE===\n[{\"dst\":\"default\",\"gateway\":\"10.0.0.1\",\"dev\":\"eth0\"}]\n",
                                "===ETHTOOL===\neth0 1000Mb/s\n",
                                "===LLDP===\n{}\n",
                                "===AVAILABILITY===\nethtool=1\nlldp=0\n",
                                "===END===\n"
                            ),
                            "log-scan" => "warning: fixture service needs attention\n",
                            "log-system-errors" => {
                                concat!(
                                    "fixture.service: failed to start fixture worker\n",
                                    "fixture.service: retry was exhausted\n",
                                )
                            }
                            "log-kernel-warnings" => "",
                            "log-auth-failures" => "sshd: failed password for fixture\n",
                            "monitoring-state" => {
                                if observed_monitoring_enabled.load(Ordering::SeqCst) {
                                    "monitoring=enabled\n"
                                } else {
                                    "monitoring=disabled\n"
                                }
                            }
                            "monitoring-enable" => {
                                if observed_monitoring_repair_succeeds.load(Ordering::SeqCst) {
                                    observed_monitoring_enabled.store(true, Ordering::SeqCst);
                                    "monitoring=enabled\n"
                                } else {
                                    "monitoring=disabled\n"
                                }
                            }
                            "monitoring-disable" => {
                                observed_monitoring_enabled.store(false, Ordering::SeqCst);
                                "monitoring=disabled\n"
                            }
                            other => panic!("unexpected SSH operation {other}"),
                        };
                        if request.operation_id.as_str() != "gadgetini-telemetry" {
                            assert_eq!(request.target_id.as_str(), "edge-one");
                        }
                        BrokerResponse::SshExecution(SshExecutionResult::new(
                            request.target_id,
                            request.operation_id,
                            0,
                            stdout.into(),
                            String::new(),
                            8,
                        ))
                    }
                    BrokerRequest::OutcomeFeedback(request) => {
                        observed_outcome_feedback_requests.fetch_add(1, Ordering::SeqCst);
                        BrokerResponse::OutcomeFeedbackAccepted(
                            OutcomeFeedbackReceipt::new(
                                request.draft.feedback_id,
                                "sha256:fixture-experience-revision",
                                false,
                            )
                            .unwrap(),
                        )
                    }
                    _ => unreachable!("fixture covers current broker requests"),
                };
                let response =
                    BrokerEnvelope::new(request.message_id, broker_identity.clone(), payload);
                let mut encoded = serde_json::to_vec(&response).unwrap();
                encoded.push(b'\n');
                broker_write.write_all(&encoded).await.unwrap();
                broker_write.flush().await.unwrap();
            }
        });
        let mut core_read = BufReader::new(core_read);

        let handshake = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:1",
                identity.clone(),
                HostRequest::Handshake(HandshakeRequest::new(
                    DIGEST,
                    BUNDLE_HOST_PROTOCOL_VERSION,
                    BUNDLE_HOST_PROTOCOL_VERSION,
                )),
            ),
        )
        .await;
        assert!(matches!(handshake.payload, HostResponse::Handshake(_)));
        let health = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:2",
                identity.clone(),
                HostRequest::Health(HealthRequest::default()),
            ),
        )
        .await;
        assert!(matches!(
            health.payload,
            HostResponse::Health(ref report) if report.status == HealthStatus::Healthy
        ));

        let context = InvocationContext::new("tenant-not-forwarded", "manager-1", "request-1")
            .with_broker_lease(
                InvocationLeaseToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            );
        let result = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:3",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.host-stats-list").unwrap(),
                    serde_json::json!({"limit": 1}),
                    context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            result.payload,
            HostResponse::GadgetResult(ref result)
                if result.output["count"] == 1 && result.output["rows"][0]["stats"]["cpu_percent"] == 12.5
        ));

        let inventory_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-2")
                .with_broker_lease(
                    InvocationLeaseToken::new("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")
                        .unwrap(),
                );
        let inventory = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:4",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.host-inventory").unwrap(),
                    serde_json::json!({"target_id": "edge-one"}),
                    inventory_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            inventory.payload,
            HostResponse::GadgetResult(ref result)
                if result.output["target_id"] == "edge-one"
                    && result.output["facts"]["hostname"] == "edge-one"
                    && result.output["duration_ms"] == 8
        ));

        let logs_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-logs")
                .with_broker_lease(
                    InvocationLeaseToken::new("DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD")
                        .unwrap(),
                );
        let logs = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:logs",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("loganalysis.inspect").unwrap(),
                    serde_json::json!({
                        "target_id": "edge-one",
                        "preset": "system-errors",
                        "limit": 10,
                    }),
                    logs_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            logs.payload,
            HostResponse::GadgetResult(ref result)
                if result.output["target_id"] == "edge-one"
                    && result.output["count"] == 2
                    && result.evidence.len() == 1
                    && result.evidence[0].metadata["operation_id"] == "log-system-errors"
                    && !result.evidence[0].passage.chars().any(char::is_control)
        ));

        let empty_logs_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-empty-logs")
                .with_broker_lease(
                    InvocationLeaseToken::new("EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE")
                        .unwrap(),
                );
        let empty_logs = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:empty-logs",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("loganalysis.inspect").unwrap(),
                    serde_json::json!({
                        "target_id": "edge-one",
                        "preset": "kernel-warnings",
                        "limit": 10,
                    }),
                    empty_logs_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            empty_logs.payload,
            HostResponse::GadgetResult(ref result)
                if result.output["count"] == 0
                    && result.output["evidence"]["status"] == "empty"
                    && result.evidence.is_empty()
        ));

        let cooling_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-cooling")
                .with_broker_lease(
                    InvocationLeaseToken::new("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF")
                        .unwrap(),
                );
        let cooling = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:cooling",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.gadgetini-telemetry-collect").unwrap(),
                    serde_json::json!({
                        "gadgetini_id": "gadgetini-one",
                        "parent_target_id": "edge-one",
                        "attach_mode": "direct",
                    }),
                    cooling_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            cooling.payload,
            HostResponse::Error(ref error) if error.code.as_str() == "gadgetini-not-attached"
        ));
        assert_eq!(cooling_writes.load(Ordering::SeqCst), 0);

        let attach_context = InvocationContext::new(
            "tenant-not-forwarded",
            "manager-1",
            "request-cooling-attach",
        )
        .with_broker_lease(
            InvocationLeaseToken::new("HHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHH").unwrap(),
        );
        let attach = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:cooling-attach",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.gadgetini-attach").unwrap(),
                    serde_json::json!({
                        "gadgetini_id": "gadgetini-one",
                        "parent_target_id": "edge-one",
                        "attach_mode": "direct",
                    }),
                    attach_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            attach.payload,
            HostResponse::GadgetResult(ref result)
                if result.output["gadgetini_id"] == "gadgetini-one"
                    && result.output["parent_target_id"] == "edge-one"
                    && result.output["relation_revision"]
                        == "11111111-2222-4333-8444-555555555555"
                    && result.output["attached"] == true
                    && result.output["observation"]["observation_status"] == "observed"
                    && result.output["observation"]["stats"]["coolant_leak_detected"] == true
        ));
        assert_eq!(cooling_writes.load(Ordering::SeqCst), 7);

        let retire_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-parent-retire")
                .with_broker_lease(
                    InvocationLeaseToken::new("IIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIII")
                        .unwrap(),
                );
        let retire = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:parent-retire",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.target-retire").unwrap(),
                    serde_json::json!({"target_id": "edge-one"}),
                    retire_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            retire.payload,
            HostResponse::Error(ref error)
                if error.code.as_str() == "gadgetini-relationship-active"
        ));

        let child_retire_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-child-retire")
                .with_broker_lease(
                    InvocationLeaseToken::new("KKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKK")
                        .unwrap(),
                );
        let child_retire = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:child-retire",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.target-retire").unwrap(),
                    serde_json::json!({"target_id": "gadgetini-one"}),
                    child_retire_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            child_retire.payload,
            HostResponse::Error(ref error)
                if error.code.as_str() == "gadgetini-relationship-active"
        ));

        let usb_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-cooling-usb")
                .with_broker_lease(
                    InvocationLeaseToken::new("GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG")
                        .unwrap(),
                );
        let usb = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:cooling-usb",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.gadgetini-attach").unwrap(),
                    serde_json::json!({
                        "gadgetini_id": "gadgetini-one",
                        "parent_target_id": "edge-one",
                        "attach_mode": "usb",
                    }),
                    usb_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            usb.payload,
            HostResponse::Error(ref error) if error.code.as_str() == "gadgetini-already-attached"
        ));
        assert_eq!(cooling_writes.load(Ordering::SeqCst), 7);

        let stale_detach_context = InvocationContext::new(
            "tenant-not-forwarded",
            "manager-1",
            "request-cooling-stale-detach",
        )
        .with_broker_lease(
            InvocationLeaseToken::new("LLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLLL").unwrap(),
        );
        let stale_detach = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:cooling-stale-detach",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.gadgetini-detach").unwrap(),
                    serde_json::json!({
                        "gadgetini_id": "gadgetini-one",
                        "parent_target_id": "edge-one",
                        "expected_revision": "99999999-2222-4333-8444-555555555555",
                    }),
                    stale_detach_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            stale_detach.payload,
            HostResponse::Error(ref error)
                if error.code.as_str() == "gadgetini-revision-conflict"
        ));
        assert!(cooling_relation
            .lock()
            .expect("cooling relation fixture lock poisoned")
            .is_some());

        let detach_context = InvocationContext::new(
            "tenant-not-forwarded",
            "manager-1",
            "request-cooling-detach",
        )
        .with_broker_lease(
            InvocationLeaseToken::new("JJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJJ").unwrap(),
        );
        let detach = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:cooling-detach",
                identity.clone(),
                HostRequest::InvokeGadget(GadgetInvocation::new(
                    GadgetName::new("server.gadgetini-detach").unwrap(),
                    serde_json::json!({
                        "gadgetini_id": "gadgetini-one",
                        "parent_target_id": "edge-one",
                        "expected_revision": "11111111-2222-4333-8444-555555555555",
                    }),
                    detach_context,
                )),
            ),
        )
        .await;
        assert!(matches!(
            detach.payload,
            HostResponse::GadgetResult(ref result)
                if result.output["detached"] == true
                    && result.output["history_preserved"] == true
        ));
        assert!(cooling_relation
            .lock()
            .expect("cooling relation fixture lock poisoned")
            .is_none());

        let job_context = InvocationContext::new("tenant-not-forwarded", "manager-1", "request-3")
            .with_broker_lease(
                InvocationLeaseToken::new("CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC").unwrap(),
            );
        let accepted = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:5",
                identity.clone(),
                HostRequest::StartJob(JobStartRequest::new(
                    LocalId::new("server-duty-cycle").unwrap(),
                    serde_json::json!({
                        "target_id":"edge-one",
                        "target_revision":"11111111-1111-4111-8111-111111111111",
                        "context_query_id":"fixture-context-query",
                        "context_revision":"sha256:fixture-context-revision",
                        "used_citation_id":"fixture-citation",
                        "used_source_revision":"sha256:fixture-source-revision",
                    }),
                    job_context,
                )),
            ),
        )
        .await;
        let job_id = match accepted.payload {
            HostResponse::JobAccepted(accepted) => accepted.job_id,
            other => panic!("expected job acceptance, got {other:?}"),
        };
        let mut terminal = None;
        for attempt in 0..40 {
            let polled = tokio::time::timeout(
                Duration::from_secs(2),
                exchange(
                    &mut core_read,
                    &mut core_write,
                    ProtocolEnvelope::new(
                        format!("broker-test:poll-{attempt}"),
                        identity.clone(),
                        HostRequest::PollJob(JobPollRequest::new(job_id.clone())),
                    ),
                ),
            )
            .await
            .expect("knowledge-linked duty-cycle polling must not deadlock");
            let HostResponse::JobStatus(report) = polled.payload else {
                panic!("expected job status");
            };
            if report.status == JobStatus::Succeeded {
                terminal = Some(report);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let terminal = terminal.expect("duty-cycle job reaches a terminal state");
        assert_eq!(terminal.progress.unwrap()["completed"], 6);
        let result = terminal.result.unwrap();
        assert_eq!(result.output["steps"].as_object().unwrap().len(), 6);
        assert_eq!(
            result.output["steps"]["server.monitoring-observe"]["status"],
            "action_required"
        );
        assert_eq!(
            result.output["steps"]["server.monitoring-observe"]["incident_id"],
            monitoring_incident_id
        );
        assert_eq!(
            result.output["steps"]["server.monitoring-repair"]["status"],
            "recovered"
        );
        assert!(result.output.get("before").is_some());
        assert_eq!(result.output["after"]["health_status"], "healthy");
        assert_eq!(
            result.output["steps"]["server.monitoring-repair"]["experience"]["state"],
            "recorded"
        );
        assert_eq!(
            result.output["steps"]["server.monitoring-repair"]["experience"]["outcome_tracking"]
                ["state"],
            "linked"
        );
        assert_eq!(result.outcomes[0].status, ObservedOutcome::Succeeded);
        assert!(monitoring_enabled.load(Ordering::SeqCst));
        assert!(!monitoring_alert_active.load(Ordering::SeqCst));
        assert_eq!(outcome_feedback_requests.load(Ordering::SeqCst), 1);

        monitoring_enabled.store(false, Ordering::SeqCst);
        monitoring_repair_succeeds.store(false, Ordering::SeqCst);
        let failed_context =
            InvocationContext::new("tenant-not-forwarded", "manager-1", "request-failed-repair")
                .with_broker_lease(
                    InvocationLeaseToken::new("NNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNN")
                        .unwrap(),
                );
        let accepted = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:failed-repair",
                identity.clone(),
                HostRequest::StartJob(JobStartRequest::new(
                    LocalId::new("server-duty-cycle").unwrap(),
                    serde_json::json!({
                        "target_id":"edge-one",
                        "target_revision":"22222222-2222-4222-8222-222222222222",
                        "context_query_id":"fixture-failed-context-query",
                        "context_revision":"sha256:fixture-failed-context-revision",
                        "used_citation_id":"fixture-failed-citation",
                        "used_source_revision":"sha256:fixture-failed-source-revision",
                    }),
                    failed_context,
                )),
            ),
        )
        .await;
        let failed_job_id = match accepted.payload {
            HostResponse::JobAccepted(accepted) => accepted.job_id,
            other => panic!("expected failed-repair job acceptance, got {other:?}"),
        };
        let mut failed_terminal = None;
        for attempt in 0..40 {
            let polled = tokio::time::timeout(
                Duration::from_secs(2),
                exchange(
                    &mut core_read,
                    &mut core_write,
                    ProtocolEnvelope::new(
                        format!("broker-test:failed-poll-{attempt}"),
                        identity.clone(),
                        HostRequest::PollJob(JobPollRequest::new(failed_job_id.clone())),
                    ),
                ),
            )
            .await
            .expect("failed repair polling must not deadlock");
            let HostResponse::JobStatus(report) = polled.payload else {
                panic!("expected failed repair job status");
            };
            if report.status == JobStatus::Failed {
                failed_terminal = Some(report);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let failed_report = failed_terminal.expect("failed repair reaches a terminal state");
        assert_eq!(failed_report.status, JobStatus::Failed);
        let failed_result = failed_report.result.unwrap();
        assert_eq!(
            failed_result.output["error"]["code"],
            "monitoring-recovery-safe-stopped"
        );
        assert!(monitoring_alert_active.load(Ordering::SeqCst));
        assert_eq!(
            outcome_feedback_requests.load(Ordering::SeqCst),
            1,
            "failed repair must not call Core Outcome feedback"
        );

        let invalid_enrollment = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:invalid-enrollment",
                identity.clone(),
                HostRequest::StartJob(JobStartRequest::new(
                    LocalId::new("server-enrollment").unwrap(),
                    serde_json::json!({
                        "target_id": "edge-one",
                        "enrollment_id": "not-a-uuid",
                    }),
                    InvocationContext::new(
                        "tenant-not-forwarded",
                        "manager-1",
                        "request-invalid-enrollment",
                    )
                    .with_broker_lease(
                        InvocationLeaseToken::new("MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM")
                            .unwrap(),
                    ),
                )),
            ),
        )
        .await;
        assert!(matches!(
            invalid_enrollment.payload,
            HostResponse::Error(ref error) if error.code.as_str() == "invalid-arguments"
        ));

        let shutdown = exchange(
            &mut core_read,
            &mut core_write,
            ProtocolEnvelope::new(
                "broker-test:6",
                identity,
                HostRequest::Shutdown(ShutdownRequest::default()),
            ),
        )
        .await;
        assert!(matches!(shutdown.payload, HostResponse::Acknowledgement(_)));
        server.await.unwrap();
        broker.await.unwrap();
    }

    async fn exchange<R, W>(
        reader: &mut BufReader<R>,
        writer: &mut W,
        request: ProtocolEnvelope<HostRequest>,
    ) -> ProtocolEnvelope<HostResponse>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut encoded = serde_json::to_vec(&request).unwrap();
        encoded.push(b'\n');
        writer.write_all(&encoded).await.unwrap();
        writer.flush().await.unwrap();

        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).await.unwrap();
        serde_json::from_slice(&line).unwrap()
    }
}
