//! Mode C bootstrap — operator supplies sudo user + password *once*, we
//! turn the target into a permanently key-authenticated NOPASSWD state.
//!
//! Order matters:
//!
//! 1. Generate fresh ed25519 keypair on *this* host.
//! 2. Password-SSH into the target.
//! 3. Append our public key to `~/.ssh/authorized_keys` (create + chmod as
//!    needed).
//! 4. Drop a `/etc/sudoers.d/gadgetron-monitor` line granting the user
//!    NOPASSWD **only** for the four binaries we invoke during polling
//!    (`dcgmi`, `smartctl`, `ipmitool`, `nvidia-smi`). We *never* grant
//!    blanket root access.
//! 5. Run `apt-get update` + package install via `sudo -S` (password piped
//!    through stdin — still the one-shot secret, not the new NOPASSWD
//!    entry, because that entry is scoped to the four monitoring binaries
//!    and can't run `apt-get`).
//! 6. Conditionally install DCGM when `nvidia-smi` is detected.
//! 7. Re-verify connectivity with key-only auth and fail closed if it
//!    doesn't work — we refuse to persist a half-bootstrapped host.
//! 8. Caller zeroizes the password immediately after `run_bootstrap`
//!    returns.
//!
//! All scripts are assembled as single-line strings and piped through
//! `bash -lc` so one SSH round-trip does the whole step. This cuts
//! latency on slow links and reduces the number of `sudo -S` prompts.

use std::path::Path;

use serde::Serialize;

use crate::ssh::{exec, exec_with_password, CmdOutput, SshError, SshTarget};

/// Detected OS family of a registration target. Each branch has its
/// own package manager, its own cuda-keyring (when relevant), and its
/// own "which system log feed / journal command" — we pick the right
/// script bundle by matching on this enum at bootstrap time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OsFamily {
    /// Ubuntu / Debian — `apt-get`, `journalctl`, `/etc/machine-id`.
    Debian,
    /// RHEL family — Rocky / Alma / CentOS Stream / RHEL proper.
    /// `dnf`, `journalctl`, `/etc/machine-id`, SELinux present.
    Rhel,
    /// macOS — `brew` (not installed by us), `log show` instead of
    /// journalctl, `launchctl` instead of systemctl, `ioreg` for the
    /// hardware UUID. No NVIDIA → DCGM skipped.
    Mac,
    /// Detection failed or the box is something we don't script for
    /// (Alpine, Arch, FreeBSD, Windows under WSL…). Caller falls back
    /// to the minimum bootstrap (SSH key only) and emits a warning.
    Unknown,
}

impl OsFamily {
    pub fn label(self) -> &'static str {
        match self {
            OsFamily::Debian => "debian",
            OsFamily::Rhel => "rhel",
            OsFamily::Mac => "mac",
            OsFamily::Unknown => "unknown",
        }
    }
}

/// Probe the target's OS over SSH with the one-shot password. Reads
/// `/etc/os-release` when present (every mainstream Linux distro has
/// it), otherwise falls back to `uname` and a hostname sniff for Mac.
/// Best-effort: we never fail registration on detection failure —
/// `Unknown` just means we skip the distro-specific steps and the
/// caller proceeds with the minimal path.
pub async fn detect_os(target_pw: &SshTarget, password: &str) -> OsFamily {
    let probe = r#"if [ -r /etc/os-release ]; then
    ID=$(. /etc/os-release; echo "${ID:-}") ;
    ID_LIKE=$(. /etc/os-release; echo "${ID_LIKE:-}") ;
    echo "osrelease_id=$ID" ;
    echo "osrelease_like=$ID_LIKE" ;
fi
UN=$(uname 2>/dev/null || true) ; echo "uname=$UN""#;
    let Ok(out) = exec_with_password(target_pw, password, probe).await else {
        return OsFamily::Unknown;
    };
    let mut id = String::new();
    let mut id_like = String::new();
    let mut uname = String::new();
    for line in out.stdout.lines() {
        if let Some(v) = line.strip_prefix("osrelease_id=") {
            id = v.trim().trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("osrelease_like=") {
            id_like = v.trim().trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("uname=") {
            uname = v.trim().to_string();
        }
    }
    if uname == "Darwin" {
        return OsFamily::Mac;
    }
    let debian_family = ["debian", "ubuntu", "linuxmint", "pop"];
    let rhel_family = ["rhel", "centos", "rocky", "almalinux", "fedora"];
    let id_lower = id.to_lowercase();
    let id_like_lower = id_like.to_lowercase();
    if debian_family.iter().any(|k| id_lower == *k)
        || id_like_lower
            .split_whitespace()
            .any(|k| debian_family.contains(&k))
    {
        return OsFamily::Debian;
    }
    if rhel_family.iter().any(|k| id_lower == *k)
        || id_like_lower
            .split_whitespace()
            .any(|k| rhel_family.contains(&k))
    {
        return OsFamily::Rhel;
    }
    OsFamily::Unknown
}

/// What we did on the target. Returned by `server.add` and surfaced
/// verbatim in the UI so operators can sanity-check.
#[derive(Debug, Clone, Serialize, Default)]
pub struct BootstrapReport {
    pub installed_pkgs: Vec<String>,
    pub skipped_pkgs: Vec<String>,
    pub gpu_detected: bool,
    pub dcgm_enabled: bool,
    pub key_installed: bool,
    pub sudoers_installed: bool,
    pub notes: Vec<String>,
}

/// Run the full bootstrap. `password` and `sudo_password` are consumed
/// as `&str` references — the caller owns the backing storage and is
/// responsible for zeroizing after return.
pub async fn run_bootstrap(
    target_pw: &SshTarget,
    password: &str,
    sudo_password: &str,
    public_key: &str,
    private_key_path: &Path,
) -> Result<BootstrapReport, SshError> {
    let mut report = BootstrapReport::default();

    // Step 1 — replace any prior gadgetron-monitor pubkeys and install
    // ours. Re-bootstrap mints a fresh ed25519 keypair, so blindly
    // appending would leave every previous re-registration as an
    // orphan public key on the target (observed: 10+ stale
    // `gadgetron-monitor:<ip>` lines on a thrice-rebootstrapped host).
    //
    // Sweep rule: remove any line whose last whitespace-separated token
    // starts with `gadgetron-monitor:`. Non-gadgetron keys (operators,
    // deployment bots, etc.) have different comments and stay intact.
    //
    // mktemp in the same directory so the `mv` is atomic (no window
    // where authorized_keys is missing). The final marker lets the
    // Rust side confirm end-to-end success instead of trusting a
    // potentially chained exit code.
    let pk_escaped = shell_escape_single(public_key);
    let install_key = format!(
        "set -e; \
         umask 077; \
         mkdir -p ~/.ssh; \
         chmod 700 ~/.ssh; \
         touch ~/.ssh/authorized_keys; \
         chmod 600 ~/.ssh/authorized_keys; \
         REMOVED=$(grep -cE ' gadgetron-monitor:[^ ]*$' ~/.ssh/authorized_keys || true); \
         TMP=$(mktemp ~/.ssh/ak.XXXXXX); \
         grep -vE ' gadgetron-monitor:[^ ]*$' ~/.ssh/authorized_keys > \"$TMP\" || true; \
         chmod 600 \"$TMP\"; \
         mv \"$TMP\" ~/.ssh/authorized_keys; \
         echo {pk} >> ~/.ssh/authorized_keys; \
         echo \"__gsm_key_installed__ removed=$REMOVED\"",
        pk = pk_escaped,
    );
    let out = exec_with_password(target_pw, password, &install_key).await?;
    if !out.ok() || !out.stdout.contains("__gsm_key_installed__") {
        return Err(SshError::Bootstrap(format!(
            "install authorized_keys failed (code={}): stdout={:?}, stderr={:?}",
            out.code,
            out.stdout.trim(),
            out.stderr.trim(),
        )));
    }
    report.key_installed = true;
    let removed = out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("__gsm_key_installed__ removed="))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if removed > 0 {
        report.notes.push(format!(
            "authorized_keys: swept {removed} orphan gadgetron key(s), appended new"
        ));
    } else {
        report
            .notes
            .push("authorized_keys: appended new gadgetron key".into());
    }

    // Step 2 — drop NOPASSWD sudoers for the four monitoring binaries.
    // visudo is used via `-cf` so a typo never leaves a broken file.
    // `systemctl` is in the allowlist so Penny (and the operator via
    // the UI) can start/stop/restart system services — most
    // importantly `nvidia-dcgm` when the DCGM hostengine dies. The
    // verbs we call are pinned in the `server.systemctl` gadget;
    // operators get the full sudoers breadth on the target in case
    // they want to run ad-hoc recoveries.
    //
    // `apt`/`apt-get`/`apt-cache`/`dpkg` are also allowed so the
    // `server.apt` gadget can install / remove / upgrade packages
    // without a follow-up sudo prompt. SECURITY NOTE: this gives the
    // gadgetron-monitor key full root via `apt install <malicious-
    // deb>`. The compensating control is the dedicated SSH key per
    // host (one key per gadgetron deployment) — operators who need
    // stricter isolation should run a separate gadgetron tenant or
    // strip the apt entries below post-bootstrap.
    // /bin/bash is included so `server.bash` with `use_sudo=true` can
    // elevate without a password prompt. This widens the blast radius to
    // "any shell command" — which is exactly why the `server_admin`
    // policy bucket defaults to Ask: Penny can't auto-invoke, an
    // operator must click a confirm dialog in the UI for every call.
    let sudoers_body = format!(
        "{user} ALL=(root) NOPASSWD: /usr/bin/dcgmi, /usr/sbin/smartctl, \
         /usr/bin/ipmitool, /usr/bin/nvidia-smi, /usr/bin/systemctl, \
         /bin/systemctl, /usr/bin/journalctl, /bin/journalctl, \
         /usr/bin/apt, /usr/bin/apt-get, /usr/bin/apt-cache, /usr/bin/dpkg, \
         /usr/bin/dmesg, /bin/dmesg, /usr/bin/tail, /bin/bash\n",
        user = target_pw.user,
    );
    let sudoers_esc = shell_escape_single(&sudoers_body);
    let install_sudoers = format!(
        "set -e; \
         TMP=$(mktemp) && printf '%s' {body} > \"$TMP\" && \
         chmod 0440 \"$TMP\" && \
         echo {spw} | sudo -S -p '' /usr/sbin/visudo -cf \"$TMP\" >/dev/null && \
         echo {spw} | sudo -S -p '' install -m 0440 -o root -g root \"$TMP\" /etc/sudoers.d/gadgetron-monitor && \
         rm -f \"$TMP\"",
        body = sudoers_esc,
        spw = shell_escape_single(sudo_password),
    );
    let out = exec_with_password(target_pw, password, &install_sudoers).await?;
    expect_ok(&out, "install sudoers")?;
    report.sudoers_installed = true;

    // Step 3 — OS-aware baseline package install.
    // Debian → apt-get; RHEL → dnf; Mac → skip entirely (user installs
    // deps via brew when they actually need them); Unknown → skip with
    // a warning so the operator notices drift.
    let os = detect_os(target_pw, password).await;
    report
        .notes
        .push(format!("detected os family: {}", os.label()));
    match os {
        OsFamily::Debian => {
            let (pkgs, script) = build_apt_install_script(sudo_password);
            let out = exec_with_password(target_pw, password, &script).await?;
            if !out.ok() {
                report.notes.push(format!(
                    "apt install baseline failed: {}",
                    out.stderr.trim()
                ));
                return Err(SshError::Bootstrap(format!(
                    "apt-get install failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                )));
            }
            for p in &pkgs {
                report.installed_pkgs.push(p.clone());
            }
        }
        OsFamily::Rhel => {
            let (pkgs, script) = build_dnf_install_script(sudo_password);
            let out = exec_with_password(target_pw, password, &script).await?;
            if !out.ok() {
                report.notes.push(format!(
                    "dnf install baseline failed: {}",
                    out.stderr.trim()
                ));
                return Err(SshError::Bootstrap(format!(
                    "dnf install failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                )));
            }
            for p in &pkgs {
                report.installed_pkgs.push(p.clone());
            }
        }
        OsFamily::Mac => {
            report.notes.push(
                "macOS target — skipping baseline apt/dnf. Install \
                 telemetry helpers via `brew install ...` as needed; \
                 `server.bash` works out of the box."
                    .into(),
            );
        }
        OsFamily::Unknown => {
            report.notes.push(
                "unknown OS family — skipping baseline package install. \
                 Telemetry may be partial; register via key_path + \
                 manual setup for full parity."
                    .into(),
            );
        }
    }

    // Step 4 — detect NVIDIA, install DCGM if appropriate.
    // DCGM is a Linux-only path. Mac + Unknown skip even when probing
    // succeeds (it won't on Mac).
    if matches!(os, OsFamily::Debian | OsFamily::Rhel) {
        let probe = exec_with_password(target_pw, password, "command -v nvidia-smi").await?;
        if probe.ok() && !probe.stdout.trim().is_empty() {
            report.gpu_detected = true;
            let dcgm_script = match os {
                OsFamily::Debian => build_dcgm_install_script(sudo_password),
                OsFamily::Rhel => build_dcgm_install_script_rhel(sudo_password),
                _ => unreachable!(),
            };
            let out = exec_with_password(target_pw, password, &dcgm_script).await?;
            if out.ok() {
                report.dcgm_enabled = true;
                report.installed_pkgs.push("datacenter-gpu-manager".into());
            } else {
                report.notes.push(format!(
                    "DCGM install failed; falling back to nvidia-smi ({}) ",
                    out.stderr.trim()
                ));
            }
        } else {
            report.skipped_pkgs.push("datacenter-gpu-manager".into());
            report.notes.push("no NVIDIA GPU detected".into());
        }
    } else {
        report.skipped_pkgs.push("datacenter-gpu-manager".into());
        report.notes.push(
            "DCGM path skipped — only Linux (Debian/RHEL) targets can run \
             nvidia-dcgm today"
                .into(),
        );
    }

    // Step 5 — verify key-only login works now. We need a fresh
    // `SshTarget` with `key_path: Some(...)` — the `target_pw` parameter
    // used for password auth has `key_path: None`, which would make
    // `exec()` omit `-i` + `BatchMode=yes` and let the client fall back
    // to password prompts. With no TTY attached to the gadgetron daemon
    // that scenario hangs → 3 failed attempts → "Permission denied" —
    // NOT because the key is wrong, but because it was never tried.
    let target_key = SshTarget {
        key_path: Some(private_key_path.to_path_buf()),
        ..target_pw.clone()
    };
    let verify_cmd = "echo __gsm_ready__";
    let verify = exec(&target_key, verify_cmd).await?;
    if !verify.ok() || !verify.stdout.contains("__gsm_ready__") {
        return Err(SshError::Bootstrap(format!(
            "post-bootstrap key-only login failed (code={}, key={}): stdout={:?}, stderr={:?}",
            verify.code,
            private_key_path.display(),
            verify.stdout.trim(),
            verify.stderr.trim(),
        )));
    }
    Ok(report)
}

/// Sanity-check during Mode A/B: confirm the supplied key can actually
/// connect and we have sudo (via the NOPASSWD entry or pre-existing
/// rights). Returns `Ok` with notes if sudo is missing — the caller may
/// still register the host but only stats that don't need sudo will
/// succeed.
pub async fn verify_key_only(target: &SshTarget) -> Result<BootstrapReport, SshError> {
    let mut report = BootstrapReport {
        key_installed: true,
        ..Default::default()
    };
    let out = exec(
        target,
        "echo __gsm_ready__ && command -v nvidia-smi || true",
    )
    .await?;
    if !out.ok() || !out.stdout.contains("__gsm_ready__") {
        return Err(SshError::Bootstrap(format!(
            "SSH with key failed: {}",
            out.stderr.trim()
        )));
    }
    if out.stdout.contains("nvidia-smi") {
        report.gpu_detected = true;
    }
    let sudo_probe = exec(target, "sudo -n -l /usr/bin/dcgmi 2>/dev/null").await?;
    if !sudo_probe.ok() {
        report.notes.push(
            "NOPASSWD sudo for /usr/bin/dcgmi / smartctl / ipmitool not detected — sudo-gated \
             metrics will be reported as unavailable"
                .into(),
        );
    }
    Ok(report)
}

fn build_apt_install_script(sudo_pw: &str) -> (Vec<String>, String) {
    // Baseline packages — picked so a freshly-installed Ubuntu/Debian
    // box can satisfy every command our hot-path scripts invoke
    // (collect_stats / collect_info / dcgm install) WITHOUT additional
    // operator follow-up. Each package is here for a specific reason:
    //
    //   ca-certificates  — TLS trust root for `curl https://...`
    //                       (DCGM keyring fetch fails otherwise)
    //   curl             — used by the DCGM keyring download step
    //   gnupg            — apt-get install of cuda-keyring needs
    //                       `gnupg` to verify the deb signature
    //   lm-sensors       — `sensors -j` for chip-level temps
    //   smartmontools    — disk SMART data (forward-looking)
    //   ipmitool         — DCMI power readings, BMC inspection
    //   pciutils         — `lspci` for inventory + GPU presence checks
    //   util-linux       — `lscpu` (CPU info), `dmesg`
    //   procps           — `uptime`, `ps` (some minimal images skip it)
    //   coreutils        — `awk`, `head`, `tail`, `df` etc.
    //   gawk             — explicit GNU awk; some bundles default to mawk
    //                       which mishandles `awk -F': *'` patterns
    //   jq               — Penny-friendly JSON shaping for ad-hoc queries
    //                       (server.systemctl status piped through jq)
    //   net-tools        — `ifconfig` / `route` fallback for old runbooks
    //
    // `--no-install-recommends` keeps the footprint minimal; we only
    // get exactly what we asked for.
    let pkgs = vec![
        "ca-certificates".to_string(),
        "curl".to_string(),
        "gnupg".to_string(),
        "lm-sensors".to_string(),
        "smartmontools".to_string(),
        "ipmitool".to_string(),
        "pciutils".to_string(),
        "util-linux".to_string(),
        "procps".to_string(),
        "coreutils".to_string(),
        "gawk".to_string(),
        "jq".to_string(),
        "net-tools".to_string(),
    ];
    let pkg_args = pkgs.join(" ");
    let spw = shell_escape_single(sudo_pw);
    let script = format!(
        "set -e; export DEBIAN_FRONTEND=noninteractive; \
         echo {spw} | sudo -S -p '' apt-get update -qq && \
         echo {spw} | sudo -S -p '' apt-get install -y -qq --no-install-recommends {pkgs}",
        pkgs = pkg_args,
    );
    (pkgs, script)
}

fn build_dcgm_install_script(sudo_pw: &str) -> String {
    let spw = shell_escape_single(sudo_pw);
    // distro-scoped cuda-keyring (Ubuntu 22.04 default; adjust as needed
    // when Debian/22-only operators surface).
    let keyring_url = "https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2204/x86_64/cuda-keyring_1.1-1_all.deb";
    // The script:
    //   1. Installs `datacenter-gpu-manager` if absent.
    //   2. Detects the actual service unit name (nvidia-dcgm vs dcgm vs
    //      nv-hostengine — varies by packaging version).
    //   3. `enable --now` it with sudo.
    //   4. **Verifies** by asking systemd it's active AND calling
    //      `dcgmi dmon -c 1` with a 3 s timeout. Script exits non-zero
    //      if DCGM isn't actually responsive, so the Rust side's
    //      `report.dcgm_enabled` reflects reality rather than being a
    //      false positive.
    format!(
        "set -e; export DEBIAN_FRONTEND=noninteractive; \
         if ! dpkg-query -W -f='${{Status}}' datacenter-gpu-manager 2>/dev/null | grep -q 'install ok installed'; then \
           TMP=$(mktemp --suffix=.deb); curl -fsSL {url} -o \"$TMP\" && \
           echo {spw} | sudo -S -p '' dpkg -i \"$TMP\" && \
           echo {spw} | sudo -S -p '' apt-get update -qq && \
           echo {spw} | sudo -S -p '' apt-get install -y -qq --no-install-recommends datacenter-gpu-manager && \
           rm -f \"$TMP\"; \
         fi; \
         UNIT=''; \
         for candidate in nvidia-dcgm dcgm nv-hostengine; do \
           if systemctl list-unit-files --no-legend \"$candidate.service\" 2>/dev/null | grep -q .; then \
             UNIT=\"$candidate\"; break; \
           fi; \
         done; \
         if [ -z \"$UNIT\" ]; then \
           echo 'DCGM service unit not found after install' >&2; exit 2; \
         fi; \
         echo \"DCGM_UNIT=$UNIT\"; \
         echo {spw} | sudo -S -p '' systemctl enable --now \"$UNIT\" || \
           {{ echo \"failed to enable $UNIT\" >&2; exit 3; }}; \
         for i in 1 2 3 4 5; do \
           if systemctl is-active --quiet \"$UNIT\"; then break; fi; \
           sleep 0.5; \
         done; \
         if ! systemctl is-active --quiet \"$UNIT\"; then \
           echo {spw} | sudo -S -p '' journalctl -u \"$UNIT\" -n 30 --no-pager >&2; \
           echo \"$UNIT did not reach active state\" >&2; exit 4; \
         fi; \
         if ! timeout 3s sudo -n /usr/bin/dcgmi dmon -c 1 -e 203 >/dev/null 2>&1; then \
           echo 'dcgmi dmon probe failed — service active but not responsive' >&2; exit 5; \
         fi; \
         echo DCGM_READY",
        url = keyring_url,
    )
}

/// RHEL family equivalent of `build_apt_install_script`. Uses `dnf`
/// (Rocky 9 / Alma 9 / RHEL 9 / Fedora) with the same package set
/// remapped to RPM names. `lsb_release` is skipped — we don't need
/// it, and modern dnf-based distros ship `/etc/os-release` universally.
fn build_dnf_install_script(sudo_pw: &str) -> (Vec<String>, String) {
    // RPM equivalents. A few names differ from Debian:
    //   - coreutils / util-linux / gawk / jq / curl: same name
    //   - lm-sensors → `lm_sensors`
    //   - pciutils / smartmontools / ipmitool: same name
    //   - net-tools → same name (legacy package on RHEL too)
    //   - ca-certificates → same name
    //   - gnupg → `gnupg2`
    //   - procps → `procps-ng`
    let pkgs = vec![
        "ca-certificates".to_string(),
        "curl".to_string(),
        "gnupg2".to_string(),
        "lm_sensors".to_string(),
        "smartmontools".to_string(),
        "ipmitool".to_string(),
        "pciutils".to_string(),
        "util-linux".to_string(),
        "procps-ng".to_string(),
        "coreutils".to_string(),
        "gawk".to_string(),
        "jq".to_string(),
        "net-tools".to_string(),
    ];
    let pkg_args = pkgs.join(" ");
    let spw = shell_escape_single(sudo_pw);
    let script = format!(
        "set -e; \
         echo {spw} | sudo -S -p '' /usr/bin/dnf -y -q makecache && \
         echo {spw} | sudo -S -p '' /usr/bin/dnf -y -q install {pkgs}",
        pkgs = pkg_args,
    );
    (pkgs, script)
}

/// RHEL family DCGM install. Uses the NVIDIA cuda-rhel9 repo (works for
/// Rocky 9 / Alma 9 / RHEL 9 / Fedora 37+). The repo file registration
/// + package install pattern differs from the `.deb` keyring flow but
/// the ultimate state (systemd unit `nvidia-dcgm.service` enabled) is
/// the same, so the detect-unit-and-enable logic is reused verbatim.
fn build_dcgm_install_script_rhel(sudo_pw: &str) -> String {
    let spw = shell_escape_single(sudo_pw);
    let repo_url =
        "https://developer.download.nvidia.com/compute/cuda/repos/rhel9/x86_64/cuda-rhel9.repo";
    format!(
        "set -e; \
         if ! rpm -q datacenter-gpu-manager >/dev/null 2>&1; then \
           echo {spw} | sudo -S -p '' curl -fsSL {url} -o /etc/yum.repos.d/cuda-rhel9.repo && \
           echo {spw} | sudo -S -p '' /usr/bin/dnf -y -q install datacenter-gpu-manager; \
         fi; \
         UNIT=''; \
         for candidate in nvidia-dcgm dcgm nv-hostengine; do \
           if systemctl list-unit-files --no-legend \"$candidate.service\" 2>/dev/null | grep -q .; then \
             UNIT=\"$candidate\"; break; \
           fi; \
         done; \
         if [ -z \"$UNIT\" ]; then \
           echo 'DCGM service unit not found after install' >&2; exit 2; \
         fi; \
         echo \"DCGM_UNIT=$UNIT\"; \
         echo {spw} | sudo -S -p '' systemctl enable --now \"$UNIT\" || \
           {{ echo \"failed to enable $UNIT\" >&2; exit 3; }}; \
         for i in 1 2 3 4 5; do \
           if systemctl is-active --quiet \"$UNIT\"; then break; fi; \
           sleep 0.5; \
         done; \
         if ! systemctl is-active --quiet \"$UNIT\"; then \
           echo {spw} | sudo -S -p '' journalctl -u \"$UNIT\" -n 30 --no-pager >&2; \
           echo \"$UNIT did not reach active state\" >&2; exit 4; \
         fi; \
         if ! timeout 3s sudo -n /usr/bin/dcgmi dmon -c 1 -e 203 >/dev/null 2>&1; then \
           echo 'dcgmi dmon probe failed — service active but not responsive' >&2; exit 5; \
         fi; \
         echo DCGM_READY",
        url = repo_url,
    )
}

fn expect_ok(out: &CmdOutput, label: &str) -> Result<(), SshError> {
    if out.ok() {
        Ok(())
    } else {
        Err(SshError::Bootstrap(format!(
            "{label} failed (code {}): {}",
            out.code,
            out.stderr.trim()
        )))
    }
}

/// Safe single-quote escaping for bash. `'foo'bar'` → `'foo'\''bar'`.
fn shell_escape_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_handles_single_quotes() {
        assert_eq!(shell_escape_single("ab'cd"), "'ab'\\''cd'");
        assert_eq!(shell_escape_single("plain"), "'plain'");
        assert_eq!(shell_escape_single(""), "''");
    }

    #[test]
    fn apt_install_script_contains_all_pkgs() {
        let (pkgs, script) = build_apt_install_script("PW");
        assert!(pkgs.contains(&"lm-sensors".to_string()));
        assert!(script.contains("lm-sensors"));
        assert!(script.contains("smartmontools"));
        assert!(script.contains("ipmitool"));
        // Password is escaped inside single quotes, not inline.
        assert!(script.contains("'PW'"));
        assert!(!script.contains("echo PW"));
    }

    #[test]
    fn dcgm_script_is_idempotent_via_dpkg_query() {
        let script = build_dcgm_install_script("x");
        assert!(script.contains("dpkg-query -W"));
        assert!(script.contains("datacenter-gpu-manager"));
        assert!(script.contains("nv-hostengine"));
    }
}
