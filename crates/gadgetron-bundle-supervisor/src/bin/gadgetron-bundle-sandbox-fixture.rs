use std::{
    fs,
    io::{BufRead, Write},
    net::{SocketAddr, TcpStream},
    os::{fd::FromRawFd, unix::net::UnixStream},
    path::Path,
    time::Duration,
};

use gadgetron_bundle_sdk::{
    Acknowledgement, BrokerEnvelope, BrokerProbeRequest, BrokerRequest, BrokerResource,
    BrokerResourceReadiness, BrokerResponse, BundleId, BundleRuntimeIdentity, GadgetResult,
    HandshakeResponse, HealthReport, HealthStatus, HostError, HostRequest, HostResponse, LocalId,
    ProtocolEnvelope, BUNDLE_HOST_PROTOCOL_VERSION,
};
use semver::Version;

fn main() {
    let mut args = std::env::args().skip(1);
    let host_probe = args.next().unwrap_or_default();
    let degraded = args.any(|value| value == "degraded");
    let digest = std::env::var("GADGETRON_BUNDLE_MANIFEST_SHA256")
        .expect("sandbox fixture requires the package manifest digest");
    let identity = BundleRuntimeIdentity::new(
        BundleId::new("sandbox-fixture").unwrap(),
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
    );
    let broker_fd = std::env::var("GADGETRON_BUNDLE_BROKER_FD")
        .expect("sandbox fixture requires the broker fd")
        .parse::<i32>()
        .expect("sandbox broker fd must be an integer");
    assert_eq!(broker_fd, 3, "sandbox broker must use fixed fd 3");
    // SAFETY: the supervisor owns and transfers exactly this Unix socket fd to
    // the runtime. This process takes ownership once.
    let broker = unsafe { UnixStream::from_raw_fd(broker_fd) };
    let mut broker_reader = std::io::BufReader::new(
        broker
            .try_clone()
            .expect("clone sandbox broker channel for reading"),
    );
    let mut broker_writer = broker;
    let mut broker_message_id = 1_u64;
    let mut handshaken = false;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("read protocol frame");
        let request: ProtocolEnvelope<HostRequest> =
            serde_json::from_str(&line).expect("parse protocol frame");
        request
            .validate_routing(&identity, BUNDLE_HOST_PROTOCOL_VERSION)
            .expect("validate protocol routing");
        request.payload.validate().expect("validate request");
        let stop = matches!(request.payload, HostRequest::Shutdown(_));
        let payload = match request.payload {
            HostRequest::Handshake(handshake)
                if handshake.package_manifest_sha256 == digest
                    && (handshake.protocol_min..=handshake.protocol_max)
                        .contains(&BUNDLE_HOST_PROTOCOL_VERSION) =>
            {
                handshaken = true;
                HostResponse::Handshake(HandshakeResponse::new(
                    digest.clone(),
                    BUNDLE_HOST_PROTOCOL_VERSION,
                ))
            }
            HostRequest::Handshake(_) => host_error(
                "handshake-rejected",
                "fixture manifest digest or protocol mismatch",
            ),
            _ if !handshaken => host_error(
                "handshake-required",
                "fixture requires handshake before other requests",
            ),
            HostRequest::Health(_) if degraded => HostResponse::Health(HealthReport::with_message(
                HealthStatus::Degraded,
                "fixture requested degraded",
            )),
            HostRequest::Health(_) => HostResponse::Health(HealthReport::healthy()),
            HostRequest::InvokeGadget(invocation)
                if invocation.gadget.as_str() == "fixture.probe" =>
            {
                HostResponse::GadgetResult(GadgetResult::new(run_probe(
                    &host_probe,
                    &identity,
                    &mut broker_reader,
                    &mut broker_writer,
                    &mut broker_message_id,
                )))
            }
            HostRequest::Shutdown(_) => {
                HostResponse::Acknowledgement(Acknowledgement::new("sandbox fixture stopping"))
            }
            HostRequest::InvokeGadget(_)
            | HostRequest::StartJob(_)
            | HostRequest::PollJob(_)
            | HostRequest::CancelJob(_) => {
                host_error("unsupported-request", "fixture request is not supported")
            }
            _ => host_error("unsupported-request", "fixture request is not supported"),
        };
        let response = ProtocolEnvelope::new(request.message_id, identity.clone(), payload);
        response.payload.validate().expect("validate response");
        serde_json::to_writer(&mut stdout, &response).expect("serialize response");
        stdout.write_all(b"\n").expect("terminate response");
        stdout.flush().expect("flush response");
        if stop && handshaken {
            break;
        }
    }
}

fn host_error(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(HostError::new(LocalId::new(code).unwrap(), message, false))
}

fn run_probe(
    host_probe: &str,
    identity: &BundleRuntimeIdentity,
    broker_reader: &mut std::io::BufReader<UnixStream>,
    broker_writer: &mut UnixStream,
    broker_message_id: &mut u64,
) -> serde_json::Value {
    let host_network_reachable = host_probe
        .parse::<SocketAddr>()
        .ok()
        .and_then(|address| TcpStream::connect_timeout(&address, Duration::from_millis(300)).ok())
        .is_some();
    let package_write_blocked = fs::write("/bundle/forbidden", b"no").is_err();
    let host_usr_write_blocked = fs::write("/usr/forbidden", b"no").is_err();
    let state_write_succeeded = fs::write("/data/probe", b"sandbox-state").is_ok();
    let mount_blocked = {
        let _ = fs::create_dir_all("/tmp/mount-probe");
        let source = b"tmpfs\0";
        let target = b"/tmp/mount-probe\0";
        let filesystem = b"tmpfs\0";
        unsafe {
            libc::mount(
                source.as_ptr().cast(),
                target.as_ptr().cast(),
                filesystem.as_ptr().cast(),
                0,
                std::ptr::null(),
            ) != 0
        }
    };
    let capabilities_empty = fs::read_to_string("/proc/self/status")
        .map(|status| {
            ["CapInh:", "CapPrm:", "CapEff:", "CapBnd:", "CapAmb:"]
                .iter()
                .all(|field| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix(field))
                        .map(str::trim)
                        .is_some_and(|value| value.chars().all(|character| character == '0'))
                })
        })
        .unwrap_or(false);
    let (address_space_limit, open_file_limit, cpu_limit) = read_limits();
    let (broker_channel_available, broker_probe_ready) =
        broker_probe(identity, broker_reader, broker_writer, broker_message_id);
    serde_json::json!({
        "host_home_hidden": !Path::new("/home").exists(),
        "parent_database_url_hidden": std::env::var_os("GADGETRON_DATABASE_URL").is_none(),
        "runtime_path_minimal": std::env::var("PATH").ok().as_deref() == Some("/usr/bin"),
        "package_write_blocked": package_write_blocked,
        "host_usr_write_blocked": host_usr_write_blocked,
        "state_write_succeeded": state_write_succeeded,
        "host_network_blocked": !host_network_reachable,
        "mount_blocked": mount_blocked,
        "capabilities_empty": capabilities_empty,
        "pid_namespace_init": std::process::id() == 1,
        "broker_fd_is_fixed": std::env::var("GADGETRON_BUNDLE_BROKER_FD").ok().as_deref() == Some("3"),
        "broker_channel_available": broker_channel_available,
        "broker_probe_ready": broker_probe_ready,
        "address_space_limit": address_space_limit,
        "open_file_limit": open_file_limit,
        "cpu_limit": cpu_limit,
    })
}

fn broker_probe(
    identity: &BundleRuntimeIdentity,
    reader: &mut std::io::BufReader<UnixStream>,
    writer: &mut UnixStream,
    next_message_id: &mut u64,
) -> (bool, bool) {
    let message_id = format!("fixture:{}", *next_message_id);
    *next_message_id = next_message_id.saturating_add(1);
    let request = BrokerEnvelope::new(
        message_id.clone(),
        identity.clone(),
        BrokerRequest::Probe(BrokerProbeRequest::new(
            LocalId::new("broker-probe").unwrap(),
            BrokerResource::database_table("sandbox_broker").unwrap(),
        )),
    );
    if serde_json::to_writer(&mut *writer, &request).is_err()
        || writer.write_all(b"\n").is_err()
        || writer.flush().is_err()
    {
        return (false, false);
    }
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .ok()
        .filter(|read| *read > 0)
        .is_none()
    {
        return (false, false);
    }
    let Ok(response) = serde_json::from_str::<BrokerEnvelope<BrokerResponse>>(&line) else {
        return (false, false);
    };
    if response.validate_routing(identity).is_err()
        || response.message_id != message_id
        || response.payload.validate().is_err()
    {
        return (false, false);
    }
    match response.payload {
        BrokerResponse::Probe(result) => (
            true,
            matches!(result.readiness, BrokerResourceReadiness::Ready),
        ),
        BrokerResponse::Error(_) => (true, false),
        _ => (false, false),
    }
}

fn read_limits() -> (u64, u64, u64) {
    unsafe fn current(resource: libc::__rlimit_resource_t) -> u64 {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(resource, &mut limit) } == 0 {
            limit.rlim_cur
        } else {
            u64::MAX
        }
    }
    unsafe {
        (
            current(libc::RLIMIT_AS),
            current(libc::RLIMIT_NOFILE),
            current(libc::RLIMIT_CPU),
        )
    }
}
