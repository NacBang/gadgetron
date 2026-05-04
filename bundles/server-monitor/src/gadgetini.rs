use std::path::PathBuf;
use std::process::Stdio;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::ssh::{exec, shell_escape_single, CmdOutput, SshError, SshTarget};

pub const FACTORY_PASSWORD_ENV: &str = "GADGETRON_GADGETINI_FACTORY_PASSWORD";
pub const DEFAULT_USB_IPV6: &str = "fd12:3456:789a:1::2";
pub const DEFAULT_USB_PARENT_IFACE: &str = "usb0";

pub const GADGETINI_REDIS_KEYS: [&str; 12] = [
    "air_humit",
    "air_temp",
    "chassis_stabil",
    "coolant_delta_t1",
    "coolant_leak",
    "coolant_level",
    "coolant_temp",
    "coolant_temp_inlet1",
    "coolant_temp_inlet2",
    "coolant_temp_outlet1",
    "coolant_temp_outlet2",
    "host_stat",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GadgetiniRecord {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub host_name: Option<String>,
    #[serde(default = "default_ssh_user")]
    pub ssh_user: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    pub parent_iface: String,
    pub ipv6_link_local: String,
    #[serde(default)]
    pub mac: Option<String>,
    pub key_path: PathBuf,
    #[serde(default)]
    pub web_port: Option<u16>,
    #[serde(default)]
    pub last_ok_at: Option<DateTime<Utc>>,
}

fn default_enabled() -> bool {
    true
}

fn default_ssh_user() -> String {
    "gadgetini".into()
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GadgetiniStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_humidity_pct: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_temp_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chassis_stable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_delta_t_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_leak_detected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_level_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_temp_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_temp_inlet1_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_temp_inlet2_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_temp_outlet1_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coolant_temp_outlet2_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_status_code: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedGadgetiniStats {
    pub stats: GadgetiniStats,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GadgetiniDiscovery {
    pub host_name: Option<String>,
    pub ipv6_link_local: Option<String>,
    pub parent_iface: Option<String>,
    pub mac: Option<String>,
}

pub async fn discover_gadgetini(parent: &SshTarget) -> Result<GadgetiniDiscovery, SshError> {
    let script = r#"set +e
        RAW=$(getent hosts gadgetini.local 2>/dev/null | awk '/fe80:/ {print $1; exit}')
        if [ -z "$RAW" ]; then
            RAW=$(getent ahosts gadgetini.local 2>/dev/null | awk '/fe80:/ {print $1; exit}')
        fi
        IP="$RAW"
        IFACE=""
        case "$RAW" in
            *%*) IP="${RAW%%%*}"; IFACE="${RAW#*%}" ;;
        esac
        if [ -z "$IFACE" ] && [ -n "$IP" ]; then
            IFACE=$(ip -6 neigh show 2>/dev/null | awk -v ip="$IP" '$1 == ip { for (i=1;i<=NF;i++) if ($i=="dev") {print $(i+1); exit} }')
        fi
        MAC=""
        if [ -n "$IP" ]; then
            MAC=$(ip -6 neigh show 2>/dev/null | awk -v ip="$IP" '$1 == ip { for (i=1;i<=NF;i++) if ($i=="lladdr") {print $(i+1); exit} }')
        fi
        printf 'host_name=%s\n' "gadgetini.local"
        printf 'ipv6_link_local=%s\n' "$RAW"
        printf 'parent_iface=%s\n' "$IFACE"
        printf 'mac=%s\n' "$MAC"
    "#;
    let out = exec(parent, script).await?;
    if !out.ok() && out.stdout.is_empty() {
        return Err(SshError::Failed {
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        });
    }
    Ok(parse_discovery_stdout(&out.stdout))
}

pub fn parse_discovery_stdout(stdout: &str) -> GadgetiniDiscovery {
    let mut found = GadgetiniDiscovery::default();
    for line in stdout.lines() {
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let value = raw_value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "host_name" => found.host_name = Some(value.to_string()),
            "ipv6_link_local" => {
                if let Some((ip, iface)) = value.split_once('%') {
                    if !ip.trim().is_empty() {
                        found.ipv6_link_local = Some(ip.trim().to_string());
                    }
                    if found.parent_iface.is_none() && !iface.trim().is_empty() {
                        found.parent_iface = Some(iface.trim().to_string());
                    }
                } else {
                    found.ipv6_link_local = Some(value.to_string());
                }
            }
            "parent_iface" => found.parent_iface = Some(value.to_string()),
            "mac" => found.mac = Some(value.to_string()),
            _ => {}
        }
    }
    found
}

pub fn parse_redis_mget_stdout(stdout: &str) -> ParsedGadgetiniStats {
    let values: Vec<&str> = stdout.lines().collect();
    parse_redis_mget_values(&values)
}

pub fn parse_redis_mget_values(values: &[&str]) -> ParsedGadgetiniStats {
    let mut parsed = ParsedGadgetiniStats::default();

    parsed.stats.air_humidity_pct = parse_f32(values, 0, "air_humit", &mut parsed.warnings);
    parsed.stats.air_temp_c = parse_f32(values, 1, "air_temp", &mut parsed.warnings);
    parsed.stats.chassis_stable =
        parse_bool_flag(values, 2, "chassis_stabil", &mut parsed.warnings);
    parsed.stats.coolant_delta_t_c = parse_f32(values, 3, "coolant_delta_t1", &mut parsed.warnings);
    parsed.stats.coolant_leak_detected =
        parse_bool_flag(values, 4, "coolant_leak", &mut parsed.warnings);
    parsed.stats.coolant_level_ok =
        parse_bool_flag(values, 5, "coolant_level", &mut parsed.warnings);
    parsed.stats.coolant_temp_c = parse_f32(values, 6, "coolant_temp", &mut parsed.warnings);
    parsed.stats.coolant_temp_inlet1_c =
        parse_f32(values, 7, "coolant_temp_inlet1", &mut parsed.warnings);
    parsed.stats.coolant_temp_inlet2_c =
        parse_f32(values, 8, "coolant_temp_inlet2", &mut parsed.warnings);
    parsed.stats.coolant_temp_outlet1_c =
        parse_f32(values, 9, "coolant_temp_outlet1", &mut parsed.warnings);
    parsed.stats.coolant_temp_outlet2_c =
        parse_f32(values, 10, "coolant_temp_outlet2", &mut parsed.warnings);
    parsed.stats.host_status_code = parse_i64(values, 11, "host_stat", &mut parsed.warnings);

    parsed
}

fn parse_f32(values: &[&str], index: usize, key: &str, warnings: &mut Vec<String>) -> Option<f32> {
    let raw = values.get(index)?.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<f32>() {
        Ok(v) => Some(v),
        Err(_) => {
            warnings.push(format!(
                "gadgetini redis key {key} value {raw:?} is not a float"
            ));
            None
        }
    }
}

fn parse_i64(values: &[&str], index: usize, key: &str, warnings: &mut Vec<String>) -> Option<i64> {
    let raw = values.get(index)?.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<i64>() {
        Ok(v) => Some(v),
        Err(_) => {
            warnings.push(format!(
                "gadgetini redis key {key} value {raw:?} is not an integer"
            ));
            None
        }
    }
}

fn parse_bool_flag(
    values: &[&str],
    index: usize,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<bool> {
    parse_i64(values, index, key, warnings).map(|v| v != 0)
}

pub fn build_child_proxy_command(
    parent: &SshTarget,
    child_ipv6_link_local: &str,
    parent_iface: &str,
) -> String {
    build_child_proxy_command_with_port(parent, child_ipv6_link_local, parent_iface, 22)
}

fn build_child_proxy_command_with_port(
    parent: &SshTarget,
    child_ipv6_link_local: &str,
    parent_iface: &str,
    child_ssh_port: u16,
) -> String {
    let scoped_child = child_proxy_address(child_ipv6_link_local, parent_iface);
    let mut argv: Vec<String> = vec![
        "ssh".into(),
        "-p".into(),
        parent.port.to_string(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", parent.known_hosts.display()),
        "-o".into(),
        "ConnectTimeout=8".into(),
    ];
    if let Some(key_path) = &parent.key_path {
        argv.extend([
            "-i".into(),
            key_path.display().to_string(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            "BatchMode=yes".into(),
        ]);
    }
    argv.push(format!("{}@{}", parent.user, parent.host));
    argv.push("nc".into());
    argv.push("-6".into());
    argv.push(scoped_child);
    argv.push(child_ssh_port.to_string());
    argv.into_iter()
        .map(|arg| shell_word(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn redis_mget_command() -> String {
    format!("redis-cli --raw MGET {}", GADGETINI_REDIS_KEYS.join(" "))
}

fn child_proxy_address(child_ipv6: &str, parent_iface: &str) -> String {
    let trimmed = child_ipv6.trim().trim_matches(['[', ']']);
    let (addr, explicit_iface) = trimmed
        .split_once('%')
        .map(|(addr, iface)| (addr, Some(iface)))
        .unwrap_or((trimmed, None));
    if addr.to_ascii_lowercase().starts_with("fe80:") {
        let iface = explicit_iface.unwrap_or(parent_iface).trim();
        if iface.is_empty() {
            addr.to_string()
        } else {
            format!("{addr}%%{iface}")
        }
    } else {
        addr.to_string()
    }
}

pub fn build_child_ssh_argv(
    parent: &SshTarget,
    record: &GadgetiniRecord,
    cmd: &str,
) -> Vec<String> {
    let proxy_command = build_child_proxy_command_with_port(
        parent,
        &record.ipv6_link_local,
        &record.parent_iface,
        record.ssh_port,
    );
    let child_host = record
        .host_name
        .as_deref()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or(&record.ipv6_link_local);

    vec![
        "-p".into(),
        record.ssh_port.to_string(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", parent.known_hosts.display()),
        "-o".into(),
        "ConnectTimeout=8".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-i".into(),
        record.key_path.display().to_string(),
        "-o".into(),
        format!("ProxyCommand={proxy_command}"),
        format!("{}@{}", record.ssh_user, child_host),
        format!("bash -lc {}", shell_escape_single(cmd)),
    ]
}

pub fn build_child_password_sshpass_argv(
    parent: &SshTarget,
    record: &GadgetiniRecord,
    cmd: &str,
) -> Vec<String> {
    let proxy_command = build_child_proxy_command_with_port(
        parent,
        &record.ipv6_link_local,
        &record.parent_iface,
        record.ssh_port,
    );
    let child_host = record
        .host_name
        .as_deref()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or(&record.ipv6_link_local);

    vec![
        "-e".into(),
        "ssh".into(),
        "-p".into(),
        record.ssh_port.to_string(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", parent.known_hosts.display()),
        "-o".into(),
        "ConnectTimeout=8".into(),
        "-o".into(),
        "PubkeyAuthentication=no".into(),
        "-o".into(),
        "PreferredAuthentications=password".into(),
        "-o".into(),
        format!("ProxyCommand={proxy_command}"),
        format!("{}@{}", record.ssh_user, child_host),
        format!("bash -lc {}", shell_escape_single(cmd)),
    ]
}

pub async fn collect_gadgetini_stats(
    parent: &SshTarget,
    record: &GadgetiniRecord,
) -> Result<ParsedGadgetiniStats, SshError> {
    let out = exec_child(parent, record, &redis_mget_command()).await?;
    if !out.ok() && out.stdout.is_empty() {
        return Err(SshError::Failed {
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        });
    }
    let mut parsed = parse_redis_mget_stdout(&out.stdout);
    if !out.stderr.trim().is_empty() {
        parsed
            .warnings
            .push(format!("gadgetini stderr: {}", out.stderr.trim()));
    }
    Ok(parsed)
}

pub async fn install_child_key_with_password(
    parent: &SshTarget,
    record: &GadgetiniRecord,
    password: &str,
    pubkey: &str,
) -> Result<CmdOutput, SshError> {
    if Command::new("sshpass").arg("-V").output().await.is_err() {
        return Err(SshError::SshpassMissing);
    }
    let script = install_child_key_script(pubkey);
    exec_child_with_password(parent, record, password, &script).await
}

pub fn install_child_key_script(pubkey: &str) -> String {
    let pubkey_q = shell_escape_single(pubkey);
    format!(
        "read -r GADGETRON_GADGETINI_PASSWORD || true; \
         umask 077; mkdir -p ~/.ssh; touch ~/.ssh/authorized_keys; \
         grep -qxF {pubkey_q} ~/.ssh/authorized_keys || printf '%s\\n' {pubkey_q} >> ~/.ssh/authorized_keys; \
         chmod 700 ~/.ssh; chmod 600 ~/.ssh/authorized_keys; \
         (printf '%s\\n' \"$GADGETRON_GADGETINI_PASSWORD\" | sudo -S -p '' ip link set wlan0 down || \
          sudo -n ip link set wlan0 down || ip link set wlan0 down || nmcli radio wifi off) >/dev/null 2>&1 || true"
    )
}

pub async fn disable_child_wlan0(
    parent: &SshTarget,
    record: &GadgetiniRecord,
) -> Result<CmdOutput, SshError> {
    exec_child(parent, record, wlan0_disable_command()).await
}

pub fn wlan0_disable_command() -> &'static str {
    "(sudo -n ip link set wlan0 down || ip link set wlan0 down || nmcli radio wifi off) >/dev/null 2>&1 || true"
}

async fn exec_child(
    parent: &SshTarget,
    record: &GadgetiniRecord,
    cmd: &str,
) -> Result<CmdOutput, SshError> {
    let argv = build_child_ssh_argv(parent, record, cmd);
    let output = Command::new("ssh")
        .args(&argv)
        .output()
        .await
        .map_err(|e| SshError::Io(format!("spawn gadgetini ssh: {e}")))?;
    Ok(CmdOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

async fn exec_child_with_password(
    parent: &SshTarget,
    record: &GadgetiniRecord,
    password: &str,
    cmd: &str,
) -> Result<CmdOutput, SshError> {
    let argv = build_child_password_sshpass_argv(parent, record, cmd);
    let mut child = Command::new("sshpass")
        .args(&argv)
        .env("SSHPASS", password)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SshError::Io(format!("spawn gadgetini sshpass: {e}")))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(password.as_bytes())
            .await
            .map_err(|e| SshError::Io(format!("write gadgetini sshpass stdin: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| SshError::Io(format!("write gadgetini sshpass stdin: {e}")))?;
    }
    drop(child.stdin.take());
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| SshError::Io(format!("wait gadgetini sshpass: {e}")))?;
    Ok(CmdOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '=' | '%' | '@')
        })
    {
        return s.to_string();
    }
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
    use std::path::PathBuf;

    use super::*;
    use crate::ssh::SshTarget;

    #[test]
    fn parses_known_redis_values_into_cooling_stats() {
        let parsed = parse_redis_mget_stdout(
            "\
21.0
34.0
1
0.8
0
1
-27.9
39.2
40.9
40.0
40.8
0
",
        );

        assert_eq!(parsed.warnings, Vec::<String>::new());
        assert_eq!(parsed.stats.air_humidity_pct, Some(21.0));
        assert_eq!(parsed.stats.air_temp_c, Some(34.0));
        assert_eq!(parsed.stats.chassis_stable, Some(true));
        assert_eq!(parsed.stats.coolant_delta_t_c, Some(0.8));
        assert_eq!(parsed.stats.coolant_leak_detected, Some(false));
        assert_eq!(parsed.stats.coolant_level_ok, Some(true));
        assert_eq!(parsed.stats.coolant_temp_c, Some(-27.9));
        assert_eq!(parsed.stats.coolant_temp_inlet1_c, Some(39.2));
        assert_eq!(parsed.stats.coolant_temp_inlet2_c, Some(40.9));
        assert_eq!(parsed.stats.coolant_temp_outlet1_c, Some(40.0));
        assert_eq!(parsed.stats.coolant_temp_outlet2_c, Some(40.8));
        assert_eq!(parsed.stats.host_status_code, Some(0));
    }

    #[test]
    fn parse_keeps_partial_values_and_warns_on_bad_lines() {
        let redis_lines = [
            "",
            "bad",
            "1",
            "",
            "0",
            "",
            "",
            "39.2",
            "",
            "",
            "40.8",
            "not-an-int",
        ];
        let parsed = parse_redis_mget_values(&redis_lines);

        assert_eq!(parsed.stats.air_humidity_pct, None);
        assert_eq!(parsed.stats.air_temp_c, None);
        assert_eq!(parsed.stats.chassis_stable, Some(true));
        assert_eq!(parsed.stats.coolant_leak_detected, Some(false));
        assert_eq!(parsed.stats.coolant_temp_inlet1_c, Some(39.2));
        assert_eq!(parsed.stats.coolant_temp_outlet2_c, Some(40.8));
        assert_eq!(parsed.stats.host_status_code, None);
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("air_temp") && w.contains("bad")));
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("host_stat") && w.contains("not-an-int")));
    }

    #[test]
    fn proxy_command_escapes_ipv6_scope_for_openssh() {
        let parent = SshTarget {
            host: "192.168.1.166".into(),
            user: "deepgadget".into(),
            port: 2200,
            key_path: Some(PathBuf::from("/tmp/parent-key")),
            known_hosts: PathBuf::from("/tmp/known-hosts"),
        };

        let command =
            build_child_proxy_command(&parent, "fe80::584d:7732:805c:a8f9", "enp3s0f1np1");

        assert!(command.contains("deepgadget@192.168.1.166"));
        assert!(command.contains("-p 2200"));
        assert!(command.contains("nc -6 fe80::584d:7732:805c:a8f9%%enp3s0f1np1 22"));
        assert!(!command.contains("fe80::584d:7732:805c:a8f9%enp3s0f1np1 22"));
    }

    #[test]
    fn proxy_command_uses_usb_ula_without_scope() {
        let parent = SshTarget {
            host: "192.168.1.166".into(),
            user: "deepgadget".into(),
            port: 22,
            key_path: Some(PathBuf::from("/tmp/parent-key")),
            known_hosts: PathBuf::from("/tmp/known-hosts"),
        };

        let command = build_child_proxy_command(&parent, "fd12:3456:789a:1::2", "usb0");

        assert!(command.contains("nc -6 fd12:3456:789a:1::2 22"));
        assert!(!command.contains("fd12:3456:789a:1::2%usb0"));
        assert!(!command.contains("fd12:3456:789a:1::2%%usb0"));
    }

    #[test]
    fn child_ssh_argv_uses_child_key_and_parent_proxy() {
        let parent = SshTarget {
            host: "192.168.1.166".into(),
            user: "deepgadget".into(),
            port: 22,
            key_path: Some(PathBuf::from("/tmp/parent-key")),
            known_hosts: PathBuf::from("/tmp/known-hosts"),
        };
        let record = GadgetiniRecord {
            enabled: true,
            host_name: Some("gadgetini.local".into()),
            ssh_user: "gadgetini".into(),
            ssh_port: 2222,
            parent_iface: "enp3s0f1np1".into(),
            ipv6_link_local: "fe80::584d:7732:805c:a8f9".into(),
            mac: Some("d8:3a:dd:71:ee:b5".into()),
            key_path: PathBuf::from("/tmp/gadgetini-key"),
            web_port: None,
            last_ok_at: None,
        };

        let argv = build_child_ssh_argv(&parent, &record, "redis-cli ping");

        assert!(argv.windows(2).any(|w| w == ["-i", "/tmp/gadgetini-key"]));
        assert!(argv.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(argv.iter().any(|arg| arg.starts_with("ProxyCommand=")
            && arg.contains("deepgadget@192.168.1.166")
            && arg.contains("nc -6 fe80::584d:7732:805c:a8f9%%enp3s0f1np1 2222")));
        assert!(argv.contains(&"gadgetini@gadgetini.local".to_string()));
        assert!(argv.iter().any(|arg| arg.contains("redis-cli ping")));
    }

    #[test]
    fn redis_mget_command_uses_fixed_key_order() {
        assert_eq!(
            redis_mget_command(),
            "redis-cli --raw MGET air_humit air_temp chassis_stabil coolant_delta_t1 coolant_leak coolant_level coolant_temp coolant_temp_inlet1 coolant_temp_inlet2 coolant_temp_outlet1 coolant_temp_outlet2 host_stat"
        );
    }

    #[test]
    fn install_script_turns_off_wlan0_best_effort() {
        let script = install_child_key_script("ssh-ed25519 AAAAtest");

        assert!(script.contains("authorized_keys"));
        assert!(script.contains("sudo -S"));
        assert!(script.contains("wlan0"));
        assert!(script.contains("ip link set wlan0 down"));
        assert!(script.contains("nmcli radio wifi off"));
    }

    #[test]
    fn child_password_argv_never_contains_password() {
        let parent = SshTarget {
            host: "192.168.1.166".into(),
            user: "deepgadget".into(),
            port: 22,
            key_path: Some(PathBuf::from("/tmp/parent-key")),
            known_hosts: PathBuf::from("/tmp/known-hosts"),
        };
        let record = GadgetiniRecord {
            enabled: true,
            host_name: Some("gadgetini.local".into()),
            ssh_user: "gadgetini".into(),
            ssh_port: 22,
            parent_iface: "enp3s0f1np1".into(),
            ipv6_link_local: "fe80::584d:7732:805c:a8f9".into(),
            mac: None,
            key_path: PathBuf::from("/tmp/gadgetini-key"),
            web_port: None,
            last_ok_at: None,
        };

        let argv = build_child_password_sshpass_argv(&parent, &record, "echo ok");
        let joined = argv.join(" ");

        assert_eq!(argv.first().map(String::as_str), Some("-e"));
        assert!(joined.contains("PreferredAuthentications=password"));
        assert!(joined.contains("ProxyCommand="));
        assert!(!joined.contains("secret-password"));
    }

    #[test]
    fn parses_discovery_output_and_splits_ipv6_scope() {
        let found = parse_discovery_stdout(
            "\
host_name=gadgetini.local
ipv6_link_local=fe80::584d:7732:805c:a8f9%enp3s0f1np1
parent_iface=
mac=d8:3a:dd:71:ee:b5
",
        );

        assert_eq!(found.host_name.as_deref(), Some("gadgetini.local"));
        assert_eq!(
            found.ipv6_link_local.as_deref(),
            Some("fe80::584d:7732:805c:a8f9")
        );
        assert_eq!(found.parent_iface.as_deref(), Some("enp3s0f1np1"));
        assert_eq!(found.mac.as_deref(), Some("d8:3a:dd:71:ee:b5"));
    }
}
