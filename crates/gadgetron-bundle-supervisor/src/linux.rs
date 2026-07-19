use std::{
    collections::VecDeque,
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
        io::{AsRawFd, RawFd},
        net::UnixStream as StdUnixStream,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use gadgetron_bundle_host::{
    serve_broker_channel, BrokerCaller, BrokerChannelLimits, BrokerHostError, BundleBroker,
    BundleHostSession, DenyAllBundleBroker, ValidatedPackageContract,
};
use gadgetron_bundle_sdk::{
    Acknowledgement, GadgetInvocation, GadgetResult, HealthReport, HealthStatus, RelativePath,
    RuntimeKind, RuntimeTransport, ShutdownRequest,
};
use libc::{c_int, c_ulong, c_void};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, BufReader},
    net::UnixStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::timeout,
};

use crate::{error::BundleSupervisorError, spec::SandboxInitSpec, Result};

pub const INTERNAL_HELPER_MARKER: &str = "__bundle-sandbox-init";
const UNSHARE_PATH: &str = "/usr/bin/unshare";
const STDERR_RETAIN_BYTES: usize = 16 * 1024;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const BROKER_FD: RawFd = 3;
const BROKER_FD_ENV: &str = "GADGETRON_BUNDLE_BROKER_FD";

/// Linux namespace supervisor. The helper path is selected by trusted Core
/// composition, never by an install request or package manifest.
#[derive(Debug, Clone)]
pub struct LinuxSandboxSupervisor {
    helper_executable: PathBuf,
}

impl LinuxSandboxSupervisor {
    /// Use the running Core executable as the fixed sandbox helper.
    pub fn for_current_executable() -> Result<Self> {
        Self::from_trusted_helper_path(std::env::current_exe()?)
    }

    /// Test/package seam for a separately built copy of the same fixed helper.
    /// Callers must not populate this path from user or Bundle input.
    #[doc(hidden)]
    pub fn from_trusted_helper_path(path: impl AsRef<Path>) -> Result<Self> {
        if !Path::new(UNSHARE_PATH).is_file() {
            return Err(BundleSupervisorError::UnshareUnavailable);
        }
        let path = path
            .as_ref()
            .canonicalize()
            .map_err(|_| BundleSupervisorError::HelperUnavailable(path.as_ref().to_path_buf()))?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(BundleSupervisorError::HelperUnavailable(path));
        }
        Ok(Self {
            helper_executable: path,
        })
    }

    /// Launch, perform the digest-bound handshake and require healthy status.
    /// A degraded/unavailable process is terminated and never returned to the
    /// capability registry.
    pub async fn launch_and_probe(
        &self,
        package: &ValidatedPackageContract,
        package_root: impl AsRef<Path>,
        state_root: impl AsRef<Path>,
    ) -> Result<SandboxedBundle> {
        self.launch_and_probe_with_broker(
            package,
            package_root,
            state_root,
            Arc::new(DenyAllBundleBroker),
        )
        .await
    }

    /// Launch with a Core-owned policy executor on the private broker channel.
    /// The executor is selected by trusted Core composition, never by Bundle
    /// metadata. The default `launch_and_probe` path remains deny-all.
    pub async fn launch_and_probe_with_broker(
        &self,
        package: &ValidatedPackageContract,
        package_root: impl AsRef<Path>,
        state_root: impl AsRef<Path>,
        broker: Arc<dyn BundleBroker>,
    ) -> Result<SandboxedBundle> {
        let mut runtime = self
            .spawn(package, package_root.as_ref(), state_root.as_ref(), broker)
            .await?;
        if let Err(error) = runtime.session.handshake().await {
            runtime.terminate().await;
            return Err(BundleSupervisorError::ProbeFailed {
                phase: "handshake",
                error: error.to_string(),
                stderr: runtime.stderr_snapshot(),
            });
        }
        if let Err(error) = runtime.ensure_broker_live().await {
            runtime.terminate().await;
            return Err(error);
        }
        let health = runtime.session.health().await?;
        if health.status != HealthStatus::Healthy {
            let message = health
                .message
                .clone()
                .unwrap_or_else(|| "no health detail supplied".to_string());
            let status = health.status;
            runtime.terminate().await;
            return Err(BundleSupervisorError::HealthNotReady { status, message });
        }
        if let Err(error) = runtime.ensure_broker_live().await {
            runtime.terminate().await;
            return Err(error);
        }
        runtime.health = Some(health);
        Ok(runtime)
    }

    async fn spawn(
        &self,
        package: &ValidatedPackageContract,
        package_root: &Path,
        state_root: &Path,
        broker: Arc<dyn BundleBroker>,
    ) -> Result<SandboxedBundle> {
        let manifest = package.manifest();
        match manifest.runtime.kind {
            RuntimeKind::Subprocess => {}
            _ => return Err(BundleSupervisorError::UnsupportedRuntime),
        }
        match manifest.runtime.transport {
            RuntimeTransport::JsonRpcStdio => {}
            _ => return Err(BundleSupervisorError::UnsupportedRuntime),
        }
        if !manifest.runtime.egress.allow.is_empty() {
            return Err(BundleSupervisorError::UnsupportedEgress);
        }

        let package_root = package_root.canonicalize()?;
        let entry_relative = RelativePath::new(manifest.runtime.entry.clone())?;
        let entry_source = package_root.join(entry_relative.as_str()).canonicalize()?;
        if !entry_source.starts_with(&package_root) {
            return Err(BundleSupervisorError::InvalidEntry(entry_source));
        }
        let entry_metadata = fs::metadata(&entry_source)?;
        if !entry_metadata.is_file() || entry_metadata.permissions().mode() & 0o111 == 0 {
            return Err(BundleSupervisorError::InvalidEntry(entry_source));
        }
        let expected_digest = manifest
            .runtime
            .entry_sha256
            .as_deref()
            .ok_or_else(|| BundleSupervisorError::InvalidEntry(entry_source.clone()))?;
        let actual_digest = sha256_file(&entry_source)?;
        if actual_digest != expected_digest {
            return Err(BundleSupervisorError::EntryDigestMismatch {
                expected: expected_digest.to_string(),
                actual: actual_digest,
            });
        }

        fs::create_dir_all(state_root)?;
        fs::set_permissions(state_root, fs::Permissions::from_mode(0o700))?;
        let state_root = state_root.canonicalize()?;
        if !fs::metadata(&state_root)?.is_dir() {
            return Err(BundleSupervisorError::InvalidHelperSpec(
                "state root is not a directory".into(),
            ));
        }

        let sandbox_root = tempfile::Builder::new()
            .prefix("gadgetron-bundle-sandbox-")
            .tempdir()?;
        let spec = SandboxInitSpec {
            sandbox_root: sandbox_root.path().to_path_buf(),
            entry_source,
            entry_relative: entry_relative.into_inner(),
            entry_sha256: expected_digest.to_string(),
            state_root,
            args: manifest.runtime.args.clone(),
            memory_mb: manifest.runtime.limits.memory_mb,
            open_files: manifest.runtime.limits.open_files,
            cpu_seconds: manifest.runtime.limits.cpu_seconds,
            package_manifest_sha256: package.manifest_sha256().to_string(),
        };
        let encoded_spec = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&spec)?);

        let (broker_parent, broker_child) = StdUnixStream::pair()?;
        broker_parent.set_nonblocking(true)?;
        let broker_child_fd = broker_child.as_raw_fd();

        let mut command = Command::new(UNSHARE_PATH);
        command
            .args([
                "--user",
                "--map-root-user",
                "--mount",
                "--net",
                "--pid",
                "--ipc",
                "--uts",
                "--fork",
                "--kill-child=SIGKILL",
                "--propagation=private",
            ])
            .arg(&self.helper_executable)
            .arg(INTERNAL_HELPER_MARKER)
            .arg(encoded_spec)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // SAFETY: this closure runs after fork and before exec. It only uses
        // async-signal-safe fd syscalls and returns an io::Error on failure.
        unsafe {
            command
                .as_std_mut()
                .pre_exec(move || install_broker_fd(broker_child_fd, BROKER_FD));
        }

        let mut child = command.spawn()?;
        drop(broker_child);
        let stdin = child.stdin.take().ok_or_else(|| {
            BundleSupervisorError::InvalidHelperSpec("sandbox stdin was not piped".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BundleSupervisorError::InvalidHelperSpec("sandbox stdout was not piped".into())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            BundleSupervisorError::InvalidHelperSpec("sandbox stderr was not piped".into())
        })?;
        let stderr_buffer = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_task = spawn_stderr_drain(stderr, stderr_buffer.clone());
        let session = BundleHostSession::attach(stdout, stdin, package);
        let broker_channel = UnixStream::from_std(broker_parent)?;
        let broker_caller = BrokerCaller::from_package(package);
        let broker_task = tokio::spawn(serve_broker_channel(
            broker_channel,
            broker_caller,
            broker,
            BrokerChannelLimits::default(),
        ));

        Ok(SandboxedBundle {
            child,
            session,
            health: None,
            stderr_buffer,
            stderr_task,
            broker_task: Some(broker_task),
            _sandbox_root: sandbox_root,
        })
    }
}

pub struct SandboxedBundle {
    child: Child,
    session: BundleHostSession<ChildStdout, ChildStdin>,
    health: Option<HealthReport>,
    stderr_buffer: Arc<Mutex<VecDeque<u8>>>,
    stderr_task: JoinHandle<()>,
    broker_task: Option<JoinHandle<std::result::Result<(), BrokerHostError>>>,
    _sandbox_root: TempDir,
}

impl SandboxedBundle {
    pub fn health(&self) -> &HealthReport {
        self.health
            .as_ref()
            .expect("SandboxedBundle is returned only after a healthy probe")
    }

    pub async fn invoke_gadget(&mut self, invocation: GadgetInvocation) -> Result<GadgetResult> {
        self.ensure_broker_live().await?;
        let result = self.session.invoke_gadget(invocation).await;
        self.ensure_broker_live().await?;
        result.map_err(Into::into)
    }

    pub async fn start_job(
        &mut self,
        request: gadgetron_bundle_sdk::JobStartRequest,
    ) -> Result<gadgetron_bundle_sdk::JobAccepted> {
        self.ensure_broker_live().await?;
        let result = self.session.start_job(request).await;
        self.ensure_broker_live().await?;
        result.map_err(Into::into)
    }

    pub async fn poll_job(
        &mut self,
        request: gadgetron_bundle_sdk::JobPollRequest,
    ) -> Result<gadgetron_bundle_sdk::JobStatusReport> {
        self.ensure_broker_live().await?;
        let result = self.session.poll_job(request).await;
        self.ensure_broker_live().await?;
        result.map_err(Into::into)
    }

    pub async fn cancel_job(
        &mut self,
        request: gadgetron_bundle_sdk::JobCancelRequest,
    ) -> Result<gadgetron_bundle_sdk::JobStatusReport> {
        self.ensure_broker_live().await?;
        let result = self.session.cancel_job(request).await;
        self.ensure_broker_live().await?;
        result.map_err(Into::into)
    }

    pub fn stderr_snapshot(&self) -> String {
        let bytes: Vec<u8> = self
            .stderr_buffer
            .lock()
            .expect("stderr ring mutex poisoned")
            .iter()
            .copied()
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn shutdown(&mut self, reason: impl Into<String>) -> Result<Acknowledgement> {
        let acknowledgement = self
            .session
            .shutdown(ShutdownRequest::with_reason(reason.into()))
            .await?;
        match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(Ok(status)) if status.success() => {
                self.stderr_task.abort();
                self.abort_broker_task();
                Ok(acknowledgement)
            }
            Ok(Ok(status)) => Err(BundleSupervisorError::ChildExited {
                status,
                stderr: self.stderr_snapshot(),
            }),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => {
                self.terminate().await;
                Err(BundleSupervisorError::ShutdownTimeout)
            }
        }
    }

    async fn terminate(&mut self) {
        let _ = self.child.start_kill();
        let _ = timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await;
        if timeout(Duration::from_secs(1), &mut self.stderr_task)
            .await
            .is_err()
        {
            self.stderr_task.abort();
        }
        self.abort_broker_task();
    }

    async fn ensure_broker_live(&mut self) -> Result<()> {
        let is_finished = self
            .broker_task
            .as_ref()
            .map_or(true, JoinHandle::is_finished);
        if !is_finished {
            return Ok(());
        }
        let Some(task) = self.broker_task.take() else {
            return Err(BundleSupervisorError::BrokerChannelClosed);
        };
        match task.await {
            Ok(Ok(())) => Err(BundleSupervisorError::BrokerChannelClosed),
            Ok(Err(error)) => Err(BundleSupervisorError::BrokerChannel(error.to_string())),
            Err(error) => Err(BundleSupervisorError::BrokerChannel(error.to_string())),
        }
    }

    fn abort_broker_task(&mut self) {
        if let Some(task) = self.broker_task.take() {
            task.abort();
        }
    }
}

impl Drop for SandboxedBundle {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        self.stderr_task.abort();
        self.abort_broker_task();
    }
}

fn spawn_stderr_drain(
    stderr: tokio::process::ChildStderr,
    retained: Arc<Mutex<VecDeque<u8>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut chunk = [0_u8; 4096];
        while let Ok(read) = reader.read(&mut chunk).await {
            if read == 0 {
                break;
            }
            let mut ring = retained.lock().expect("stderr ring mutex poisoned");
            ring.extend(&chunk[..read]);
            while ring.len() > STDERR_RETAIN_BYTES {
                ring.pop_front();
            }
        }
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn install_broker_fd(source: RawFd, destination: RawFd) -> std::io::Result<()> {
    if source != destination {
        if unsafe { libc::dup2(source, destination) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        unsafe {
            libc::close(source);
        }
    }
    let flags = unsafe { libc::fcntl(destination, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(destination, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Entry point used only after the fixed Core executable has been placed in
/// fresh Linux namespaces by `unshare`.
pub fn run_internal_helper(encoded_spec: &str) -> Result<()> {
    let spec_bytes = URL_SAFE_NO_PAD.decode(encoded_spec)?;
    let spec: SandboxInitSpec = serde_json::from_slice(&spec_bytes)?;
    prepare_and_exec(spec)
}

fn prepare_and_exec(spec: SandboxInitSpec) -> Result<()> {
    validate_helper_spec(&spec)?;
    validate_broker_fd()?;
    make_mounts_private()?;
    mount_sandbox_root(&spec)?;
    copy_verified_entry(&spec)?;
    bind_state_root(&spec)?;
    mount_runtime_filesystems(&spec)?;
    enter_root(&spec.sandbox_root)?;
    apply_resource_limits(&spec)?;
    harden_process()?;
    verify_capabilities_are_empty()?;
    exec_runtime(&spec)
}

fn validate_broker_fd() -> Result<()> {
    let flags = unsafe { libc::fcntl(BROKER_FD, libc::F_GETFD) };
    if flags == -1 {
        return Err(BundleSupervisorError::InvalidHelperSpec(
            "fixed Bundle broker FD 3 is not open".into(),
        ));
    }
    let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(BROKER_FD, &mut metadata) } != 0 {
        return Err(BundleSupervisorError::Isolation {
            operation: "inspect Bundle broker fd",
            source: std::io::Error::last_os_error(),
        });
    }
    if metadata.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Err(BundleSupervisorError::InvalidHelperSpec(
            "fixed Bundle broker FD 3 is not a Unix socket".into(),
        ));
    }
    if unsafe { libc::fcntl(BROKER_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
        return Err(BundleSupervisorError::Isolation {
            operation: "preserve Bundle broker fd across runtime exec",
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn validate_helper_spec(spec: &SandboxInitSpec) -> Result<()> {
    RelativePath::new(spec.entry_relative.clone())?;
    if spec.entry_sha256.len() != 64
        || !spec
            .entry_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(BundleSupervisorError::InvalidHelperSpec(
            "entry SHA-256 is not canonical".into(),
        ));
    }
    if spec.package_manifest_sha256.len() != 64 {
        return Err(BundleSupervisorError::InvalidHelperSpec(
            "package manifest SHA-256 is not canonical".into(),
        ));
    }
    if spec.memory_mb == 0 || spec.open_files == 0 || spec.cpu_seconds == 0 {
        return Err(BundleSupervisorError::InvalidHelperSpec(
            "resource limits must be non-zero".into(),
        ));
    }
    Ok(())
}

fn make_mounts_private() -> Result<()> {
    mount_raw(
        None,
        Path::new("/"),
        None,
        (libc::MS_REC | libc::MS_PRIVATE) as c_ulong,
        None,
        "make mount tree private",
    )
}

fn mount_sandbox_root(spec: &SandboxInitSpec) -> Result<()> {
    let size_mb = spec.memory_mb.clamp(64, 512);
    let data = format!("size={size_mb}m,mode=0755");
    mount_raw(
        Some(OsStr::new("tmpfs")),
        &spec.sandbox_root,
        Some(OsStr::new("tmpfs")),
        (libc::MS_NOSUID | libc::MS_NODEV) as c_ulong,
        Some(OsStr::new(&data)),
        "mount private tmpfs root",
    )?;
    for relative in ["usr", "etc", "bundle", "data", "tmp", "proc"] {
        let path = spec.sandbox_root.join(relative);
        fs::create_dir_all(&path)?;
    }
    fs::set_permissions(
        spec.sandbox_root.join("bundle"),
        fs::Permissions::from_mode(0o555),
    )?;
    for (link, target) in [
        ("bin", "usr/bin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
    ] {
        std::os::unix::fs::symlink(target, spec.sandbox_root.join(link))?;
    }
    bind_read_only(Path::new("/usr"), &spec.sandbox_root.join("usr"))?;
    let loader_cache = spec.sandbox_root.join("etc/ld.so.cache");
    File::create(&loader_cache)?;
    bind_read_only(Path::new("/etc/ld.so.cache"), &loader_cache)
}

fn copy_verified_entry(spec: &SandboxInitSpec) -> Result<()> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&spec.entry_source)?;
    if !source.metadata()?.is_file() {
        return Err(BundleSupervisorError::InvalidEntry(
            spec.entry_source.clone(),
        ));
    }
    let destination = spec.sandbox_root.join("bundle").join(&spec.entry_relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o555))?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o500)
        .open(&destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
    let actual = hex::encode(hasher.finalize());
    if actual != spec.entry_sha256 {
        return Err(BundleSupervisorError::EntryDigestMismatch {
            expected: spec.entry_sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn bind_state_root(spec: &SandboxInitSpec) -> Result<()> {
    let target = spec.sandbox_root.join("data");
    bind_mount(&spec.state_root, &target)?;
    remount_bind(
        &target,
        (libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC) as c_ulong,
        "remount state root",
    )?;
    apply_mount_attributes(
        &target,
        true,
        MOUNT_ATTR_NOSUID | MOUNT_ATTR_NODEV | MOUNT_ATTR_NOEXEC,
        "harden state root recursively",
    )
}

fn mount_runtime_filesystems(spec: &SandboxInitSpec) -> Result<()> {
    let tmp_size_mb = (spec.memory_mb / 4).clamp(16, 128);
    let tmp_data = format!("size={tmp_size_mb}m,mode=0700");
    mount_raw(
        Some(OsStr::new("tmpfs")),
        &spec.sandbox_root.join("tmp"),
        Some(OsStr::new("tmpfs")),
        (libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC) as c_ulong,
        Some(OsStr::new(&tmp_data)),
        "mount private tmp",
    )?;
    mount_raw(
        Some(OsStr::new("proc")),
        &spec.sandbox_root.join("proc"),
        Some(OsStr::new("proc")),
        (libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC) as c_ulong,
        None,
        "mount sandbox proc",
    )
}

fn bind_read_only(source: &Path, target: &Path) -> Result<()> {
    bind_mount(source, target)?;
    remount_bind(
        target,
        (libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV) as c_ulong,
        "remount read-only bind",
    )?;
    if source.is_dir() {
        apply_mount_attributes(
            target,
            true,
            MOUNT_ATTR_RDONLY | MOUNT_ATTR_NOSUID | MOUNT_ATTR_NODEV,
            "harden read-only bind recursively",
        )?;
    }
    Ok(())
}

fn bind_mount(source: &Path, target: &Path) -> Result<()> {
    let flags = if source.is_dir() {
        libc::MS_BIND | libc::MS_REC
    } else {
        libc::MS_BIND
    };
    mount_raw(
        Some(source.as_os_str()),
        target,
        None,
        flags as c_ulong,
        None,
        "bind mount",
    )
    .map_err(|error| {
        BundleSupervisorError::InvalidHelperSpec(format!(
            "bind mount {source:?} -> {target:?} failed: {error}"
        ))
    })
}

const MOUNT_ATTR_RDONLY: u64 = 0x0000_0001;
const MOUNT_ATTR_NOSUID: u64 = 0x0000_0002;
const MOUNT_ATTR_NODEV: u64 = 0x0000_0004;
const MOUNT_ATTR_NOEXEC: u64 = 0x0000_0008;
const AT_RECURSIVE: c_int = 0x8000;

#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

fn apply_mount_attributes(
    target: &Path,
    recursive: bool,
    attributes: u64,
    operation: &'static str,
) -> Result<()> {
    let target = cstring(target.as_os_str())?;
    let attr = MountAttr {
        attr_set: attributes,
        attr_clr: 0,
        propagation: 0,
        userns_fd: 0,
    };
    let flags = if recursive { AT_RECURSIVE } else { 0 };
    let result = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            libc::AT_FDCWD,
            target.as_ptr(),
            flags,
            &attr as *const MountAttr,
            std::mem::size_of::<MountAttr>(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(BundleSupervisorError::Isolation {
            operation,
            source: std::io::Error::last_os_error(),
        })
    }
}

fn remount_bind(target: &Path, extra_flags: c_ulong, operation: &'static str) -> Result<()> {
    mount_raw(
        None,
        target,
        None,
        (libc::MS_BIND | libc::MS_REMOUNT) as c_ulong | extra_flags,
        None,
        operation,
    )
}

fn mount_raw(
    source: Option<&OsStr>,
    target: &Path,
    filesystem: Option<&OsStr>,
    flags: c_ulong,
    data: Option<&OsStr>,
    operation: &'static str,
) -> Result<()> {
    let source = source.map(cstring).transpose()?;
    let target = cstring(target.as_os_str())?;
    let filesystem = filesystem.map(cstring).transpose()?;
    let data = data.map(cstring).transpose()?;
    let result = unsafe {
        libc::mount(
            source.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            target.as_ptr(),
            filesystem.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            flags,
            data.as_ref()
                .map_or(std::ptr::null(), |v| v.as_ptr() as *const c_void),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(BundleSupervisorError::Isolation {
            operation,
            source: std::io::Error::last_os_error(),
        })
    }
}

fn enter_root(root: &Path) -> Result<()> {
    let root = cstring(root.as_os_str())?;
    if unsafe { libc::chroot(root.as_ptr()) } != 0 {
        return Err(BundleSupervisorError::Isolation {
            operation: "chroot",
            source: std::io::Error::last_os_error(),
        });
    }
    std::env::set_current_dir("/bundle")?;
    Ok(())
}

fn apply_resource_limits(spec: &SandboxInitSpec) -> Result<()> {
    let memory_bytes = spec.memory_mb.saturating_mul(1024 * 1024);
    set_limit(libc::RLIMIT_AS, memory_bytes, "set address-space limit")?;
    set_limit(
        libc::RLIMIT_NOFILE,
        u64::from(spec.open_files),
        "set open-file limit",
    )?;
    set_limit(
        libc::RLIMIT_CPU,
        u64::from(spec.cpu_seconds),
        "set CPU limit",
    )?;
    set_limit(libc::RLIMIT_NPROC, 64, "set process limit")?;
    set_limit(libc::RLIMIT_FSIZE, memory_bytes, "set file-size limit")?;
    set_limit(libc::RLIMIT_CORE, 0, "disable core dumps")
}

fn set_limit(
    resource: libc::__rlimit_resource_t,
    value: u64,
    operation: &'static str,
) -> Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(BundleSupervisorError::Isolation {
            operation,
            source: std::io::Error::last_os_error(),
        })
    }
}

fn harden_process() -> Result<()> {
    unsafe {
        libc::umask(0o077);
    }
    prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0, "set no_new_privs")?;
    prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0, "disable dumpability")?;

    let cap_last = fs::read_to_string("/proc/sys/kernel/cap_last_cap")
        .ok()
        .and_then(|value| value.trim().parse::<c_ulong>().ok())
        .unwrap_or(63);
    for capability in 0..=cap_last {
        prctl(
            libc::PR_CAPBSET_DROP,
            capability,
            0,
            0,
            0,
            "drop capability bounding set",
        )?;
    }
    let _ = prctl(
        libc::PR_CAP_AMBIENT,
        libc::PR_CAP_AMBIENT_CLEAR_ALL as c_ulong,
        0,
        0,
        0,
        "clear ambient capabilities",
    );

    #[repr(C)]
    struct CapabilityHeader {
        version: u32,
        pid: c_int,
    }
    #[derive(Clone, Copy)]
    #[repr(C)]
    struct CapabilityData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = CapabilityHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut data = [
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
    ];
    let result = unsafe {
        libc::syscall(
            libc::SYS_capset,
            &mut header as *mut CapabilityHeader,
            data.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(BundleSupervisorError::Isolation {
            operation: "clear capability sets",
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn verify_capabilities_are_empty() -> Result<()> {
    let status = fs::read_to_string("/proc/self/status")?;
    for field in ["CapInh:", "CapPrm:", "CapEff:", "CapBnd:", "CapAmb:"] {
        let value = status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .map(str::trim)
            .ok_or_else(|| {
                BundleSupervisorError::InvalidHelperSpec(format!(
                    "{field} missing from /proc/self/status"
                ))
            })?;
        if value.chars().any(|character| character != '0') {
            return Err(BundleSupervisorError::InvalidHelperSpec(format!(
                "{field} was not empty after capability drop"
            )));
        }
    }
    Ok(())
}

fn exec_runtime(spec: &SandboxInitSpec) -> Result<()> {
    let executable = Path::new("/bundle").join(&spec.entry_relative);
    let error = std::process::Command::new(executable)
        .args(&spec.args)
        .env_clear()
        .env("PATH", "/usr/bin")
        .env("HOME", "/data")
        .env("TMPDIR", "/tmp")
        .env(BROKER_FD_ENV, BROKER_FD.to_string())
        .env(
            "GADGETRON_BUNDLE_MANIFEST_SHA256",
            &spec.package_manifest_sha256,
        )
        .exec();
    Err(BundleSupervisorError::Isolation {
        operation: "exec verified runtime",
        source: error,
    })
}

fn prctl(
    option: c_int,
    arg2: c_ulong,
    arg3: c_ulong,
    arg4: c_ulong,
    arg5: c_ulong,
    operation: &'static str,
) -> Result<()> {
    if unsafe { libc::prctl(option, arg2, arg3, arg4, arg5) } == 0 {
        Ok(())
    } else {
        Err(BundleSupervisorError::Isolation {
            operation,
            source: std::io::Error::last_os_error(),
        })
    }
}

fn cstring(value: &OsStr) -> Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| BundleSupervisorError::InvalidHelperSpec("path contains a NUL byte".into()))
}
