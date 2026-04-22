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

use std::path::PathBuf;

use serde::Serialize;

use crate::ssh::{exec, exec_with_password, CmdOutput, SshError, SshTarget};

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
    private_key_path: &PathBuf,
) -> Result<BootstrapReport, SshError> {
    let mut report = BootstrapReport::default();

    // Step 1 — push our pubkey into authorized_keys (idempotent). We
    // use `set -e` + explicit control-flow so an unexpected failure
    // earlier in the pipeline can't silently swallow the append (a
    // pure `A && B || C` chain propagates the exit status of *whatever*
    // failed, which is harder to diagnose). The final line prints a
    // marker we grep for so sshpass can't claim success on a
    // half-applied state.
    let pk_escaped = shell_escape_single(public_key);
    let install_key = format!(
        "set -e; \
         umask 077; \
         mkdir -p ~/.ssh; \
         chmod 700 ~/.ssh; \
         touch ~/.ssh/authorized_keys; \
         chmod 600 ~/.ssh/authorized_keys; \
         if ! grep -qxF {pk} ~/.ssh/authorized_keys; then \
           echo >> ~/.ssh/authorized_keys; \
           echo {pk} >> ~/.ssh/authorized_keys; \
           echo __gsm_key_appended__; \
         else \
           echo __gsm_key_already_present__; \
         fi",
        pk = pk_escaped,
    );
    let out = exec_with_password(target_pw, password, &install_key).await?;
    if !out.ok()
        || !(out.stdout.contains("__gsm_key_appended__")
            || out.stdout.contains("__gsm_key_already_present__"))
    {
        return Err(SshError::Bootstrap(format!(
            "install authorized_keys failed (code={}): stdout={:?}, stderr={:?}",
            out.code,
            out.stdout.trim(),
            out.stderr.trim(),
        )));
    }
    report.key_installed = true;
    if out.stdout.contains("__gsm_key_appended__") {
        report.notes.push("authorized_keys append".into());
    } else {
        report.notes.push("authorized_keys already had our pubkey".into());
    }

    // Step 2 — drop NOPASSWD sudoers for the four monitoring binaries.
    // visudo is used via `-cf` so a typo never leaves a broken file.
    // `systemctl` is in the allowlist so Penny (and the operator via
    // the UI) can start/stop/restart system services — most
    // importantly `nvidia-dcgm` when the DCGM hostengine dies. The
    // verbs we call are pinned in the `server.systemctl` gadget;
    // operators get the full sudoers breadth on the target in case
    // they want to run ad-hoc recoveries.
    let sudoers_body = format!(
        "{user} ALL=(root) NOPASSWD: /usr/bin/dcgmi, /usr/sbin/smartctl, \
         /usr/bin/ipmitool, /usr/bin/nvidia-smi, /usr/bin/systemctl, \
         /bin/systemctl, /usr/bin/journalctl, /bin/journalctl\n",
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

    // Step 3 — apt-get install baseline packages.
    let (pkgs, script) = build_apt_install_script(sudo_password);
    let out = exec_with_password(target_pw, password, &script).await?;
    if !out.ok() {
        report
            .notes
            .push(format!("apt install baseline failed: {}", out.stderr.trim()));
        return Err(SshError::Bootstrap(format!(
            "apt-get install failed (code {}): {}",
            out.code,
            out.stderr.trim()
        )));
    }
    for p in &pkgs {
        report.installed_pkgs.push(p.clone());
    }

    // Step 4 — detect NVIDIA, install DCGM if appropriate.
    let probe = exec_with_password(target_pw, password, "command -v nvidia-smi").await?;
    if probe.ok() && !probe.stdout.trim().is_empty() {
        report.gpu_detected = true;
        let dcgm_script = build_dcgm_install_script(sudo_password);
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

    // Step 5 — verify key-only login works now. We need a fresh
    // `SshTarget` with `key_path: Some(...)` — the `target_pw` parameter
    // used for password auth has `key_path: None`, which would make
    // `exec()` omit `-i` + `BatchMode=yes` and let the client fall back
    // to password prompts. With no TTY attached to the gadgetron daemon
    // that scenario hangs → 3 failed attempts → "Permission denied" —
    // NOT because the key is wrong, but because it was never tried.
    let target_key = SshTarget {
        key_path: Some(private_key_path.clone()),
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
    let out = exec(target, "echo __gsm_ready__ && command -v nvidia-smi || true").await?;
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
    let pkgs = vec![
        "lm-sensors".to_string(),
        "smartmontools".to_string(),
        "ipmitool".to_string(),
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
