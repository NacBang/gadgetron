//! Regex-based classifier. Each rule is `(category, severity, label,
//! pattern)`. First match wins. Patterns target the most operator-
//! actionable kernel/driver/service events; the LLM fallback handles
//! the long tail.
//!
//! Severity rationale:
//!   `critical` — hardware fault or data-loss-imminent. Page now.
//!   `high`     — service crash / repeated correctable error / brute force.
//!   `medium`   — warning that may escalate; check next on-call rotation.
//!   `info`     — notable but not actionable in isolation.

use crate::model::{Classification, Severity};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

struct Rule {
    pattern: Regex,
    category: &'static str,
    severity: Severity,
    summary: &'static str,
    cause: &'static str,
    solution: &'static str,
    /// Optional click-to-run remediation. `tool` MUST be in the
    /// frontend whitelist (currently `server.systemctl` and
    /// `server.apt`); anything else is ignored. Use `args = {}` to
    /// signal "host_id will be filled in by the UI at click time".
    remediation: Option<fn() -> Value>,
}

static RULES: Lazy<Vec<Rule>> = Lazy::new(|| {
    let r = |p: &str| Regex::new(p).expect("static regex compiles");
    vec![
        // ── critical: hardware faults ─────────────────────────────────
        Rule {
            pattern: r(r"(?i)\bNVRM:\s+Xid\b|\bnvidia.*Xid\b"),
            category: "gpu_xid",
            severity: Severity::Critical,
            summary: "NVIDIA Xid GPU error (driver/hardware event)",
            cause: "GPU driver reports a hardware/driver-level event. \
                    Specific code (Xid 13/31/74 = MMU/uncontained ECC; \
                    Xid 79 = GPU fell off the bus). Often correlates \
                    with overheating, VRAM degradation, or PSU droop.",
            solution: "1) Check GPU temp + PSU rails; 2) Restart \
                       nvidia-dcgm; 3) If recurring, drain workload \
                       and run `nvidia-smi --query-remapped-rows`; \
                       4) RMA candidate if Xid 13/31/74 repeats.",
            remediation: Some(|| {
                json!({
                    "tool": "server.systemctl",
                    "args": { "verb": "restart", "unit": "nvidia-dcgm" },
                    "label": "nvidia-dcgm 재시작",
                })
            }),
        },
        Rule {
            pattern: r(r"(?i)Machine Check Exception|MCE:|hardware error"),
            category: "mce",
            severity: Severity::Critical,
            summary: "Machine Check Exception — CPU/memory hardware fault",
            cause: "CPU/chipset reports an unrecoverable hardware error \
                    (cache parity, memory controller, PCIe AER). NOT \
                    a software issue.",
            solution: "1) `mcelog --client` for decoded report; \
                       2) Check DIMM seating + ECC stats; \
                       3) Move workload off this host; 4) Open ticket \
                       with vendor support.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)Out of memory: Killed process|invoked oom-killer"),
            category: "oom_kill",
            severity: Severity::Critical,
            summary: "OOM killer terminated a process",
            cause: "Linux ran out of memory + swap and picked the \
                    highest-OOM-score process to kill. Usually means \
                    a leak or undersized cgroup limit.",
            solution: "1) Identify the killed PID + its cgroup; \
                       2) Add swap or raise memory.max for that \
                       cgroup; 3) Patch the leak; 4) Consider \
                       earlyoom for graceful pressure handling.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)kernel panic|Kernel BUG at"),
            category: "kernel_panic",
            severity: Severity::Critical,
            summary: "Kernel panic / BUG",
            cause: "Kernel hit an unrecoverable assertion. Likely \
                    causes: faulty driver, corrupt filesystem, hardware \
                    fault, or kernel bug.",
            solution: "1) Capture full panic via `journalctl -k -b -1`; \
                       2) Match against known kernel changelog; \
                       3) Boot prior kernel via grub if just upgraded; \
                       4) File bug with full stack.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)nvme.*(I/O Error|controller is down|reset|failed)"),
            category: "nvme_failure",
            severity: Severity::Critical,
            summary: "NVMe device error",
            cause: "NVMe controller reset or returned I/O error. May \
                    indicate firmware bug, overheating, wear-out, or \
                    PCIe link instability.",
            solution: "1) `smartctl -a /dev/nvme0` — check media \
                       wearout + critical warnings; 2) Check namespace \
                       temperature; 3) Consider firmware update; \
                       4) Replace if SMART says FAILING.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)EXT4-fs error|XFS.*Internal error|btrfs.*ERROR"),
            category: "fs_error",
            severity: Severity::Critical,
            summary: "Filesystem corruption / I/O error",
            cause: "Filesystem detected on-disk inconsistency or I/O \
                    failure underneath. The mount may have been \
                    remounted read-only as protection.",
            solution: "1) Schedule downtime; 2) `umount` then `fsck \
                       -y`; 3) For btrfs use `btrfs check`; \
                       4) Restore from backup if metadata damage; \
                       5) Investigate underlying disk SMART.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)Uncorrectable.*ECC|DRAM uncorrectable|Hardware Error.*Severity 3"),
            category: "ecc_uncorrectable",
            severity: Severity::Critical,
            summary: "Uncorrectable ECC memory error",
            cause: "RAM produced a multi-bit error that ECC couldn't \
                    correct. Workload data is already corrupt.",
            solution: "1) Drain workload from this host; 2) Identify \
                       failing DIMM via `edac-util` or `dmidecode -t \
                       memory`; 3) Replace DIMM; 4) Run memtest86+ \
                       on full bank before returning to service.",
            remediation: None,
        },
        // ── high: service crashes & repeating errors ─────────────────
        Rule {
            pattern: r(r"(?i)segfault at .* ip|general protection fault"),
            category: "segfault",
            severity: Severity::High,
            summary: "Process segfault / GPF",
            cause: "Process accessed invalid memory. Common causes: \
                    application bug, mismatched library version, \
                    memory corruption from upstream.",
            solution: "1) Check core dump path (`coredumpctl list`); \
                       2) Get bt with gdb on the binary; 3) Update \
                       to latest version; 4) If repeats systematically, \
                       file upstream bug.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)\bsystemd\[\d+\]:.*(Failed|failed with result)"),
            category: "service_failure",
            severity: Severity::High,
            summary: "systemd unit failure",
            cause: "A systemd-managed service exited non-zero or crashed. \
                    Restart policy may be looping if Restart=always.",
            solution: "1) `systemctl status <unit>` for exit code; \
                       2) `journalctl -u <unit> -n 200` for stderr; \
                       3) Fix root cause; 4) `systemctl restart <unit>` \
                       once resolved.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)nvidia-smi.*has failed|NVRM:.*GPU.*has fallen off the bus"),
            category: "gpu_lost",
            severity: Severity::Critical,
            summary: "GPU fell off the PCIe bus",
            cause: "GPU stopped responding on PCIe — usually after \
                    hardware fault, severe thermal event, or PSU \
                    instability. Requires reboot to recover.",
            solution: "1) Drain workload; 2) Reboot; 3) Check PCIe \
                       link width with `lspci -vvv`; 4) Inspect PSU \
                       12V rail; 5) RMA if persistent across reboots.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)Correctable.*ECC|DRAM correctable"),
            category: "ecc_correctable",
            severity: Severity::High,
            summary: "Correctable ECC error (memory degrading)",
            cause: "ECC caught + corrected a single-bit error. One-off \
                    is normal background; rising rate signals an aging \
                    DIMM that will eventually produce uncorrectable \
                    errors.",
            solution: "1) Track CE rate over time (`edac-util`); \
                       2) Schedule DIMM replacement during maintenance \
                       window if rate > 100/day per DIMM.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)thermal.*throttling|CPU\d+: Core temperature above threshold"),
            category: "thermal",
            severity: Severity::High,
            summary: "Thermal throttling / overtemp",
            cause: "CPU/GPU exceeded thermal limit and clocked down to \
                    protect itself. Usually airflow blocked, fan failure, \
                    or thermal paste degradation.",
            solution: "1) Check `sensors` for chip temps; 2) Inspect \
                       fans (RPM, dust); 3) Verify chassis airflow; \
                       4) Re-paste CPU if 3+ years old; 5) Check \
                       ambient room temp.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)smartd.*(FAILING_NOW|Currently failed)"),
            category: "smart_fail",
            severity: Severity::Critical,
            summary: "SMART pre-failure attribute",
            cause: "Disk firmware reports a SMART attribute past its \
                    failure threshold. Disk WILL fail soon.",
            solution: "1) Backup data NOW; 2) `smartctl -a /dev/<disk>` \
                       for full attributes; 3) RMA / replace ASAP; \
                       4) Don't wait for the read errors to start.",
            remediation: None,
        },
        Rule {
            pattern: r(
                r"(?i)smartd.*Device:\s+(/dev/\S+).*?(Currently unreadable \(pending\) sectors|Offline uncorrectable sectors)",
            ),
            category: "smartd_disk_health",
            severity: Severity::Critical,
            summary: "SMART disk media errors",
            cause: "smartd reports pending or offline-uncorrectable sectors. \
                    The disk has unreadable media and should be treated as a \
                    near-term data-loss risk.",
            solution: "1) Back up or evacuate data now; 2) Run `smartctl -a \
                       <device>` to capture the full report; 3) Replace the \
                       disk; 4) Verify array/filesystem health after replacement.",
            remediation: None,
        },
        Rule {
            pattern: r(
                r"(?i)smartd.*(run-parts: .*/10mail exited with return code 1|mailx or mailutils package|does not have /usr/bin/mail)",
            ),
            category: "smartd_alert_delivery",
            severity: Severity::Medium,
            summary: "smartd alert email delivery failed",
            cause: "smartd detected a condition worth notifying, but the host \
                    cannot send the configured warning email because the mail \
                    helper is missing or failing.",
            solution: "Install `mailx` or `mailutils`, or reconfigure smartd \
                       warning delivery to the team's alerting path. This does \
                       not fix the underlying disk warning.",
            remediation: None,
        },
        // ── high: security / auth ─────────────────────────────────────
        Rule {
            pattern: r(r"(?i)sshd\[\d+\]:.*Failed password.*from"),
            category: "ssh_auth_fail",
            severity: Severity::High,
            summary: "SSH login failure (possible brute-force)",
            cause: "Someone is trying SSH passwords. If from a single \
                    IP at high rate, it's a brute-force attack. If \
                    from internal IP, could be a stale credential.",
            solution: "1) Check rate from source IP; 2) Enable \
                       fail2ban or sshd `MaxAuthTries=3`; 3) Disable \
                       password auth if all users have keys \
                       (`PasswordAuthentication no`); 4) Block \
                       offending IP at firewall.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)sudo:.*authentication failure"),
            category: "sudo_fail",
            severity: Severity::High,
            summary: "sudo authentication failure",
            cause: "User typed wrong sudo password. Single occurrence \
                    is a typo; repeated failures from same user could \
                    be a compromised session or forgotten password.",
            solution: "1) Confirm with the user; 2) Check `last` for \
                       unusual login sources; 3) Force password reset \
                       if account compromise suspected.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)pam_unix\(.*\):.*authentication failure"),
            category: "pam_fail",
            severity: Severity::Medium,
            summary: "PAM authentication failure",
            cause: "PAM stack rejected a credential. Could be web \
                    panel, su, sudo, or any PAM-authenticated service.",
            solution: "1) Identify the service from the log line; \
                       2) Same drill as ssh_auth_fail / sudo_fail.",
            remediation: None,
        },
        // ── medium: warnings to watch ─────────────────────────────────
        Rule {
            pattern: r(r"(?i)NIC link is down|link down|carrier lost"),
            category: "link_down",
            severity: Severity::Medium,
            summary: "Network interface link down",
            cause: "Physical link dropped. Cable, switch port, NIC, or \
                    PHY negotiation issue.",
            solution: "1) Check `ethtool <iface>` link state + speed; \
                       2) Reseat cable; 3) Try a different switch port; \
                       4) Replace cable if intermittent.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)Disk write error|Buffer I/O error on device"),
            category: "disk_io_warn",
            severity: Severity::Medium,
            summary: "Disk I/O warning",
            cause: "Block layer reports a write error to a specific \
                    sector. Often a precursor to filesystem errors \
                    or SMART pre-fail.",
            solution: "1) `smartctl -a /dev/<disk>`; 2) Check `dmesg` \
                       for the affected sector; 3) Plan replacement.",
            remediation: None,
        },
        Rule {
            pattern: r(r"(?i)docker.*killed|containerd.*OOM"),
            category: "container_oom",
            severity: Severity::Medium,
            summary: "Container OOM",
            cause: "Container hit its memory cgroup limit and a \
                    process inside was killed. Application sized too \
                    big for the limit, or a leak.",
            solution: "1) `docker stats` for live limits; 2) Raise \
                       container --memory; 3) Profile the workload.",
            remediation: None,
        },
        // ── info: notable but routine ─────────────────────────────────
        Rule {
            pattern: r(r"(?i)Started Daily apt|Reboot scheduled"),
            category: "system_routine",
            severity: Severity::Info,
            summary: "Routine system event",
            cause: "Scheduled background activity (apt timer, reboot \
                    schedule, systemd timer firing).",
            solution: "No action required.",
            remediation: None,
        },
    ]
});

/// Category → wiki runbook slug mapping. When a finding matches one
/// of these categories, the UI surfaces `[[runbooks/<slug>]]` as a
/// link in the solution area so the operator sees the team's actual
/// playbook rather than the generic rule text. Missing mapping =
/// no link; rule text alone is used. Keep this conservative: add an
/// entry only after the wiki page exists and has been reviewed.
pub fn runbook_for_category(category: &str) -> Option<&'static str> {
    match category {
        "dbus_service_activation_timeout" => Some("bluez-dbus-activation-timeout"),
        // Add more as wiki runbooks land:
        // "gpu_xid" => Some("gpu-xid-triage"),
        // "oom_kill" => Some("oom-kill-response"),
        _ => None,
    }
}

/// Match a single log line against the rule list. `Some` on hit;
/// `None` means caller should fall back to LLM (or skip).
pub fn classify(line: &str) -> Option<Classification> {
    for rule in RULES.iter() {
        if rule.pattern.is_match(line) {
            // Append a runbook pointer to the solution when we have
            // one on file. The UI already renders `[[wiki links]]`
            // as clickable anchors via MarkdownText, so the operator
            // can jump from the finding card straight to the runbook.
            let solution = match runbook_for_category(rule.category) {
                Some(slug) => format!(
                    "{base}\n\n런북: [[runbooks/{slug}]]",
                    base = rule.solution,
                    slug = slug,
                ),
                None => rule.solution.to_string(),
            };
            return Some(Classification {
                severity: rule.severity,
                category: rule.category.to_string(),
                fingerprint: fingerprint_for(rule.category, line),
                summary: rule.summary.to_string(),
                cause: Some(rule.cause.to_string()),
                solution: Some(solution),
                remediation: rule.remediation.map(|f| f()),
            });
        }
    }
    None
}

fn fingerprint_for(category: &str, line: &str) -> String {
    if category == "smartd_disk_health" {
        static DEVICE_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"Device:\s+(/dev/\S+)").expect("static regex compiles"));
        if let Some(caps) = DEVICE_RE.captures(line) {
            if let Some(device) = caps.get(1) {
                return format!("{category}:{}", device.as_str());
            }
        }
    }
    category.to_string()
}

/// Cheap "is this line worth sending to the LLM?" gate. Skip lines
/// that obviously aren't errors so we don't waste tokens on
/// debug/info chatter. Roughly: contains "error", "fail", "warn",
/// "panic", "abort", "fatal" case-insensitively.
pub fn looks_error_ish(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "error", "fail", "panic", "abort", "fatal", "warn", "denied", "refused",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xid_matches_critical() {
        let c =
            classify("[Tue Apr 15 10:23:01 2026] NVRM: Xid (PCI:0000:01:00): 31, pid='<unknown>'")
                .unwrap();
        assert_eq!(c.category, "gpu_xid");
        assert_eq!(c.severity, Severity::Critical);
        assert!(c.cause.is_some());
        assert!(c.solution.is_some());
        assert!(c.remediation.is_some()); // restart nvidia-dcgm
    }

    #[test]
    fn ssh_brute_matches_high() {
        let c = classify(
            "Apr 15 10:00:00 host sshd[1234]: Failed password for invalid user root from 1.2.3.4",
        )
        .unwrap();
        assert_eq!(c.category, "ssh_auth_fail");
        assert_eq!(c.severity, Severity::High);
        assert!(c.cause.is_some());
    }

    #[test]
    fn benign_lines_dont_match() {
        assert!(classify("just a normal message").is_none());
    }

    #[test]
    fn smartd_pending_sector_matches_critical_disk_health() {
        let c = classify(
            "May 03 18:09:48 dg5R-PRO6000-8 smartd[3040]: Device: /dev/sdb [SAT], 6 Currently unreadable (pending) sectors",
        )
        .unwrap();
        assert_eq!(c.category, "smartd_disk_health");
        assert_eq!(c.severity, Severity::Critical);
        assert_eq!(c.fingerprint, "smartd_disk_health:/dev/sdb");
        assert!(c.summary.contains("SMART"));
    }

    #[test]
    fn smartd_offline_uncorrectable_uses_same_device_fingerprint() {
        let c = classify(
            "May 03 18:09:48 dg5R-PRO6000-8 smartd[3040]: Device: /dev/sdb [SAT], 6 Offline uncorrectable sectors",
        )
        .unwrap();
        assert_eq!(c.category, "smartd_disk_health");
        assert_eq!(c.severity, Severity::Critical);
        assert_eq!(c.fingerprint, "smartd_disk_health:/dev/sdb");
    }

    #[test]
    fn smartd_mail_delivery_failure_is_separate_finding() {
        let c = classify(
            "May 03 14:39:48 dg5R-PRO6000-8 smartd[3040]: run-parts: /etc/smartmontools/run.d/10mail exited with return code 1",
        )
        .unwrap();
        assert_eq!(c.category, "smartd_alert_delivery");
        assert_eq!(c.severity, Severity::Medium);
        assert_eq!(c.fingerprint, "smartd_alert_delivery");
    }

    #[test]
    fn looks_error_ish_filters() {
        assert!(looks_error_ish("ERROR: something broke"));
        assert!(looks_error_ish("operation failed"));
        assert!(!looks_error_ish("everything is fine here"));
    }
}
