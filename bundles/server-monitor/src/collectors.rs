//! Remote telemetry collectors.
//!
//! Each collector is a pure function `fn(SshTarget) -> Result<Part, _>`
//! so `server.stats` can fan them out in parallel via `tokio::join!`.
//! Any single collector failing is non-fatal — it surfaces as `None` in
//! the part, and `ServerStats.warnings` gets a one-line note. This keeps
//! a box with broken `dcgmi` from making the whole status card go red
//! when everything else is fine.
//!
//! Parsers are intentionally lenient: the command output format is a
//! moving target across distros, so we only pin the JSON keys we
//! consume and ignore the rest.

use serde::Serialize;
use serde_json::Value;

use crate::gadgetini::GadgetiniStats;
use crate::ssh::{exec, SshError, SshTarget};

/// Composite snapshot returned by `server.stats`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ServerStats {
    pub cpu: Option<CpuStats>,
    pub mem: Option<MemStats>,
    pub disks: Vec<DiskStats>,
    pub temps: Vec<TempReading>,
    pub gpus: Vec<GpuStats>,
    pub power: Option<PowerStats>,
    pub network: Vec<NetworkStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gadgetini: Option<GadgetiniStats>,
    pub uptime_secs: Option<u64>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuStats {
    /// 0-100. Userland + system. Idle is subtracted on the target.
    pub util_pct: f32,
    pub load_1m: f32,
    pub load_5m: f32,
    pub cores: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

impl MemStats {
    pub fn util_pct(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f32 / self.total_bytes as f32) * 100.0
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskStats {
    pub mount: String,
    pub fs: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TempReading {
    pub chip: String,
    pub label: String,
    pub celsius: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuStats {
    pub index: u32,
    pub name: String,
    pub util_pct: Option<f32>,
    pub mem_used_mib: Option<u64>,
    pub mem_total_mib: Option<u64>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    /// `"dcgm"` or `"nvidia-smi"` — lets the UI badge richer metrics.
    pub source: &'static str,
    // DCGM-only health fields. `None` when the source is nvidia-smi or
    // the GPU doesn't report the field. Surfaced on the server card as
    // a red/amber badge when non-zero so operators notice hardware
    // trouble immediately.
    /// HBM / on-card memory temperature (°C). Separate from `temp_c`
    /// which is the SM die temperature. HBM has its own thermal budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_temp_c: Option<f32>,
    /// Volatile double-bit ECC error total. Any value ≥ 1 means the
    /// GPU memory produced an UNCORRECTABLE error — workload data is
    /// already corrupt and the card should be RMA'd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecc_dbe_total: Option<u64>,
    /// Most recent XID error code (0 = none). Non-zero values map to
    /// specific NVIDIA driver/HW events. Operators need to know the
    /// MRU value; deltas go in the per-sample timeseries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xid_last: Option<u32>,
    /// Bitmask of current clock-throttle reasons (DCGM field 112).
    /// Non-zero while the GPU is running below its requested clocks —
    /// explains "util=100 but no progress". Decoded label is in
    /// `throttle_reason_label` for direct display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttle_reasons: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttle_reason_label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerStats {
    /// IPMI DCMI reading if the BMC exposes it.
    pub psu_watts: Option<f32>,
    /// Sum of per-GPU power — useful even without IPMI.
    pub gpu_watts: Option<f32>,
}

/// Per-interface network throughput. Both throughput fields are `f64`
/// bytes/second derived from two `/proc/net/dev` samples; total counters
/// give lifetime-since-boot values for sanity checks and cumulative
/// graphs.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkStats {
    pub iface: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_bytes_total: u64,
    pub tx_bytes_total: u64,
}

/// Hardware / OS fingerprint returned by `server.info`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ServerInfo {
    pub hostname: String,
    pub kernel: String,
    pub os: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub total_ram_gb: f32,
    pub gpu_models: Vec<String>,
    pub uptime_secs: u64,
    /// Stable systemd machine-id (`/etc/machine-id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// SMBIOS hardware UUID (`/sys/class/dmi/id/product_uuid`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmi_uuid: Option<String>,
    /// Chassis serial number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmi_serial: Option<String>,
}

/// Light-weight identity capture used at register time. Does NOT need
/// to pull all the fingerprint fields `collect_info` returns — three
/// stable identifiers are enough to detect "same physical box behind
/// a recycled IP" later. Errors degrade to `None` so a host with a
/// locked-down DMI doesn't fail registration.
#[derive(Debug, Clone, Default)]
pub struct MachineIdentity {
    pub machine_id: Option<String>,
    pub dmi_uuid: Option<String>,
    pub dmi_serial: Option<String>,
    /// Output of `hostname` on the target. Used as the default alias
    /// at register time.
    pub hostname: Option<String>,
}

pub async fn collect_machine_identity(target: &SshTarget) -> MachineIdentity {
    // Each line is `KEY=value`. The Linux paths (/etc/machine-id, DMI
    // sysfs) are tried first; if we're on macOS (detected via
    // `uname`), we fall back to `ioreg` for the IOPlatformUUID +
    // IOPlatformSerialNumber. Script stays single-liner so one SSH
    // round-trip covers every OS we support.
    let script = r#"set +e
        UN=$(uname 2>/dev/null || true)
        printf 'hostname='; hostname 2>/dev/null; echo
        if [ "$UN" = "Darwin" ]; then
            # macOS — DMI sysfs doesn't exist; use ioreg instead.
            MUID=$(ioreg -rd1 -c IOPlatformExpertDevice 2>/dev/null \
                     | awk -F'"' '/IOPlatformUUID/ {print $4; exit}')
            MSER=$(ioreg -rd1 -c IOPlatformExpertDevice 2>/dev/null \
                     | awk -F'"' '/IOPlatformSerialNumber/ {print $4; exit}')
            # Treat the IOPlatformUUID as BOTH machine_id and dmi_uuid
            # for merge-matching — it's the stable identifier Apple
            # exposes and doesn't change on OS reinstall.
            printf 'machine_id='; echo "$MUID"
            printf 'dmi_uuid='; echo "$MUID"
            printf 'dmi_serial='; echo "$MSER"
        else
            printf 'machine_id='; cat /etc/machine-id 2>/dev/null || cat /var/lib/dbus/machine-id 2>/dev/null; echo
            printf 'dmi_uuid='; cat /sys/class/dmi/id/product_uuid 2>/dev/null; echo
            printf 'dmi_serial='; cat /sys/class/dmi/id/product_serial 2>/dev/null || cat /sys/class/dmi/id/chassis_serial 2>/dev/null; echo
        fi
    "#;
    let Ok(out) = exec(target, script).await else {
        return MachineIdentity::default();
    };
    let mut id = MachineIdentity::default();
    for line in out.stdout.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().to_string();
        if v.is_empty() {
            continue;
        }
        match k {
            "machine_id" => id.machine_id = Some(v),
            "dmi_uuid" => id.dmi_uuid = Some(v),
            "dmi_serial" => id.dmi_serial = Some(v),
            "hostname" => id.hostname = Some(v),
            _ => {}
        }
    }
    id
}

// ---------------------------------------------------------------------------
// Composite top-level collector
// ---------------------------------------------------------------------------

pub async fn collect_stats(target: &SshTarget) -> Result<ServerStats, SshError> {
    let mut stats = ServerStats {
        fetched_at: chrono::Utc::now(),
        ..Default::default()
    };

    // Single round-trip — one bash script that prints each section with
    // a fenced delimiter so we don't have to pay SSH per-collector.
    let script = r#"set +e
        echo '===UPTIME==='
        cat /proc/uptime 2>/dev/null | awk '{print int($1)}'
        echo '===LOAD==='
        cat /proc/loadavg 2>/dev/null
        echo '===CPUINFO==='
        nproc 2>/dev/null
        echo '===STAT0==='
        head -1 /proc/stat 2>/dev/null
        echo '===NET0==='
        cat /proc/net/dev 2>/dev/null
        sleep 0.2
        echo '===STAT1==='
        head -1 /proc/stat 2>/dev/null
        echo '===NET1==='
        cat /proc/net/dev 2>/dev/null
        echo '===MEM==='
        cat /proc/meminfo 2>/dev/null
        echo '===DF==='
        df -B1 --output=source,fstype,size,used,target -x tmpfs -x devtmpfs -x squashfs -x overlay 2>/dev/null | tail -n +2
        echo '===SENSORS==='
        command -v sensors >/dev/null && sensors -j 2>/dev/null || echo '{}'
        echo '===GPUPROBE==='
        command -v dcgmi >/dev/null && echo HAS_DCGM || true
        command -v nvidia-smi >/dev/null && echo HAS_NVSMI || true
        echo '===NVSMI==='
        command -v nvidia-smi >/dev/null && \
            nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit \
                       --format=csv,noheader,nounits 2>/dev/null || true
        echo '===IPMIPWR==='
        sudo -n /usr/bin/ipmitool dcmi power reading 2>/dev/null | grep -i 'Instantaneous' || true
        echo '===END==='
    "#;
    let out = exec(target, script).await?;
    if !out.ok() && out.stdout.is_empty() {
        return Err(SshError::Failed {
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        });
    }

    let sections = split_sections(&out.stdout);

    if let Some(s) = sections.get("UPTIME") {
        stats.uptime_secs = s.trim().parse::<u64>().ok();
    }
    // STAT0 + STAT1 are two snapshots 200 ms apart — concat them so the
    // existing `parse_cpu` delta logic still works against one string.
    let stat_combined = format!(
        "{}\n{}",
        sections.get("STAT0").map(|s| s.as_str()).unwrap_or(""),
        sections.get("STAT1").map(|s| s.as_str()).unwrap_or(""),
    );
    stats.cpu = parse_cpu(
        &stat_combined,
        sections.get("LOAD").map(|s| s.as_str()).unwrap_or(""),
        sections.get("CPUINFO").map(|s| s.as_str()).unwrap_or(""),
    );
    // NET delta shares the same 200 ms sleep as the CPU delta so we get
    // two telemetry deltas per SSH round-trip.
    stats.network = parse_network(
        sections.get("NET0").map(|s| s.as_str()).unwrap_or(""),
        sections.get("NET1").map(|s| s.as_str()).unwrap_or(""),
        0.2,
    );
    stats.mem = parse_mem(sections.get("MEM").map(|s| s.as_str()).unwrap_or(""));
    stats.disks = parse_df(sections.get("DF").map(|s| s.as_str()).unwrap_or(""));
    stats.temps = parse_sensors(sections.get("SENSORS").map(|s| s.as_str()).unwrap_or("{}"))
        .unwrap_or_default();

    let has_dcgm = sections
        .get("GPUPROBE")
        .map(|s| s.contains("HAS_DCGM"))
        .unwrap_or(false);
    let has_nvsmi = sections
        .get("GPUPROBE")
        .map(|s| s.contains("HAS_NVSMI"))
        .unwrap_or(false);

    if has_dcgm {
        match collect_dcgm(target).await {
            Ok(mut gs) if !gs.is_empty() => {
                // DCGM's `dmon` doesn't emit product names — splice
                // them in from the parallel nvidia-smi snapshot so the
                // UI can render "GPU 0 — NVIDIA RTX 4090" instead of
                // the "GPU 0" placeholder we default to in the parser.
                if has_nvsmi {
                    let name_by_idx =
                        parse_nvsmi_names(sections.get("NVSMI").map(|s| s.as_str()).unwrap_or(""));
                    for g in gs.iter_mut() {
                        if let Some(n) = name_by_idx.get(&g.index) {
                            g.name = n.clone();
                        }
                    }
                }
                stats.gpus.append(&mut gs);
            }
            Ok(_) => {
                stats
                    .warnings
                    .push("dcgmi returned no rows; falling back to nvidia-smi".into());
                if has_nvsmi {
                    stats.gpus.append(&mut parse_nvsmi(
                        sections.get("NVSMI").map(|s| s.as_str()).unwrap_or(""),
                    ));
                }
            }
            Err(e) => {
                stats.warnings.push(format!("dcgmi failed: {e}"));
                if has_nvsmi {
                    stats.gpus.append(&mut parse_nvsmi(
                        sections.get("NVSMI").map(|s| s.as_str()).unwrap_or(""),
                    ));
                }
            }
        }
    } else if has_nvsmi {
        stats.gpus.append(&mut parse_nvsmi(
            sections.get("NVSMI").map(|s| s.as_str()).unwrap_or(""),
        ));
    }

    let gpu_watts = if stats.gpus.is_empty() {
        None
    } else {
        Some(stats.gpus.iter().filter_map(|g| g.power_w).sum::<f32>())
    };
    // PSU watts: DCMI `Instantaneous power reading` is the hot path.
    // A board that returns 0 from DCMI almost always has the fallback
    // (`ipmitool sensor list`) also report `na` for the per-PSU POUT
    // sensors — and that fallback costs ~4 s per call on SSIF BMCs
    // (measured on ASUS dual-PSU boards). Running it every 1 Hz poll
    // crushes latency, so we skip it by default and surface the
    // unavailability warning instead. If a future operator needs the
    // fallback, a `[server_monitor].slow_ipmi = true` config knob is
    // the right place to opt in — not every poll.
    let psu_watts =
        parse_ipmi_dcmi_power(sections.get("IPMIPWR").map(|s| s.as_str()).unwrap_or(""));
    if psu_watts.is_some() || gpu_watts.is_some() {
        stats.power = Some(PowerStats {
            psu_watts,
            gpu_watts,
        });
    }
    if psu_watts.is_none() {
        stats.warnings.push(
            "PSU wattage unavailable — BMC's DCMI Instantaneous Power reading is 0 W on this \
             board. Common on motherboards that advertise DCMI compliance but don't wire the \
             PMBus telemetry. Per-PSU POUT sensors exist in the sensor list but typically report \
             `na`; they are NOT queried on the polling hot path because `ipmitool sensor list` \
             costs ~4 s on SSIF BMCs."
                .into(),
        );
    }

    Ok(stats)
}

pub async fn collect_info(target: &SshTarget) -> Result<ServerInfo, SshError> {
    let script = r#"set +e
        echo '===HOST==='; hostname 2>/dev/null
        echo '===KERNEL==='; uname -r 2>/dev/null
        echo '===OS==='; (grep '^PRETTY_NAME=' /etc/os-release 2>/dev/null || echo 'PRETTY_NAME=unknown') | cut -d= -f2- | tr -d '"'
        echo '===CPUMODEL==='; LC_ALL=C lscpu 2>/dev/null | awk -F': *' '/Model name/ {print $2; exit}'
        echo '===NPROC==='; nproc 2>/dev/null
        echo '===MEM==='; awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null
        echo '===UPTIME==='; cat /proc/uptime 2>/dev/null | awk '{print int($1)}'
        echo '===GPUS==='; command -v nvidia-smi >/dev/null && nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null || true
        echo '===END==='
    "#;
    let out = exec(target, script).await?;
    if !out.ok() {
        return Err(SshError::Failed {
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        });
    }
    let sections = split_sections(&out.stdout);
    let hostname = sections
        .get("HOST")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let kernel = sections
        .get("KERNEL")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let os = sections
        .get("OS")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let cpu_model = sections
        .get("CPUMODEL")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let cpu_cores = sections
        .get("NPROC")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let total_ram_gb = sections
        .get("MEM")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|kb| (kb as f32) / (1024.0 * 1024.0))
        .unwrap_or(0.0);
    let uptime_secs = sections
        .get("UPTIME")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let gpu_models = sections
        .get("GPUS")
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        })
        .unwrap_or_default();

    Ok(ServerInfo {
        hostname,
        kernel,
        os,
        cpu_model,
        cpu_cores,
        total_ram_gb,
        gpu_models,
        machine_id: None,
        dmi_uuid: None,
        dmi_serial: None,
        uptime_secs,
    })
}

// ---------------------------------------------------------------------------
// Individual parsers — each ignores shape drift beyond the keys it pins.
// ---------------------------------------------------------------------------

fn split_sections(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut buf = String::new();
    for line in stdout.lines() {
        if let Some(tag) = line.strip_prefix("===").and_then(|s| s.strip_suffix("===")) {
            if let Some(name) = current.take() {
                out.insert(name, std::mem::take(&mut buf));
            }
            if tag != "END" {
                current = Some(tag.to_string());
            }
        } else if current.is_some() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if let Some(name) = current.take() {
        out.insert(name, buf);
    }
    out
}

/// Two /proc/stat snapshots 300ms apart → CPU util via delta.
fn parse_cpu(stat_section: &str, load_section: &str, nproc_section: &str) -> Option<CpuStats> {
    let lines: Vec<&str> = stat_section
        .lines()
        .filter(|l| l.starts_with("cpu "))
        .collect();
    let (t0, i0) = parse_stat_line(lines.first()?)?;
    let (t1, i1) = parse_stat_line(lines.get(1)?)?;
    let dt = t1.saturating_sub(t0);
    let di = i1.saturating_sub(i0);
    let util_pct = if dt == 0 {
        0.0
    } else {
        (((dt - di) as f32) / (dt as f32)) * 100.0
    };
    let (load_1m, load_5m) = {
        let mut it = load_section.split_whitespace();
        let a: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let b: f32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        (a, b)
    };
    let cores: u32 = nproc_section.trim().parse().unwrap_or(0);
    Some(CpuStats {
        util_pct: util_pct.clamp(0.0, 100.0),
        load_1m,
        load_5m,
        cores,
    })
}

fn parse_stat_line(line: &str) -> Option<(u64, u64)> {
    // cpu  <user> <nice> <system> <idle> <iowait> <irq> <softirq> <steal> ...
    let parts: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    if parts.len() < 5 {
        return None;
    }
    let total: u64 = parts.iter().sum();
    let idle = parts[3] + parts.get(4).copied().unwrap_or(0); // idle + iowait
    Some((total, idle))
}

fn parse_mem(meminfo: &str) -> Option<MemStats> {
    let mut total_kb = 0u64;
    let mut available_kb = 0u64;
    let mut swap_total_kb = 0u64;
    let mut swap_free_kb = 0u64;
    for line in meminfo.lines() {
        let mut it = line.split_whitespace();
        let key = it.next()?.trim_end_matches(':');
        let val: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        match key {
            "MemTotal" => total_kb = val,
            "MemAvailable" => available_kb = val,
            "SwapTotal" => swap_total_kb = val,
            "SwapFree" => swap_free_kb = val,
            _ => {}
        }
    }
    if total_kb == 0 {
        return None;
    }
    let total_bytes = total_kb * 1024;
    let available_bytes = available_kb * 1024;
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    let swap_total_bytes = swap_total_kb * 1024;
    let swap_used_bytes = (swap_total_kb.saturating_sub(swap_free_kb)) * 1024;
    Some(MemStats {
        total_bytes,
        used_bytes,
        available_bytes,
        swap_used_bytes,
        swap_total_bytes,
    })
}

fn parse_df(df_section: &str) -> Vec<DiskStats> {
    df_section
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let fs_dev = it.next()?.to_string();
            let fstype = it.next()?.to_string();
            let total: u64 = it.next()?.parse().ok()?;
            let used: u64 = it.next()?.parse().ok()?;
            let mount = it.next()?.to_string();
            Some(DiskStats {
                mount,
                fs: format!("{fs_dev} ({fstype})"),
                total_bytes: total,
                used_bytes: used,
            })
        })
        .collect()
}

fn parse_sensors(json: &str) -> Option<Vec<TempReading>> {
    let root: Value = serde_json::from_str(json).ok()?;
    let obj = root.as_object()?;
    let mut out = Vec::new();
    for (chip, features) in obj.iter() {
        let Some(fmap) = features.as_object() else {
            continue;
        };
        for (label, value) in fmap.iter() {
            let Some(vmap) = value.as_object() else {
                continue;
            };
            // sensors -j structures look like:
            //   {"Core 0": {"temp1_input": 41.0, "temp1_max": ... }}
            // We take the first `*_input` we find.
            for (k, v) in vmap.iter() {
                if k.ends_with("_input") {
                    if let Some(c) = v.as_f64() {
                        out.push(TempReading {
                            chip: chip.clone(),
                            label: label.clone(),
                            celsius: c as f32,
                        });
                        break;
                    }
                }
            }
        }
    }
    Some(out)
}

fn parse_network(before: &str, after: &str, elapsed_secs: f64) -> Vec<NetworkStats> {
    // `/proc/net/dev` layout (after a 2-line header):
    //   Inter-|   Receive                                                |  Transmit
    //    face | bytes    packets errs drop fifo frame compressed multicast | bytes    packets errs drop fifo colls carrier compressed
    //     eth0: 12345678  ...
    //       lo: 9999      ...
    //
    // We pull `iface`, `rx_bytes` (col 1), `tx_bytes` (col 9). Anything
    // missing the 17 expected columns is skipped — `/proc/net/dev` is
    // stable across kernel versions but defensive parsing never hurts.
    let lo_before = index_net(before);
    let lo_after = index_net(after);
    let mut out = Vec::new();
    for (iface, (rx_after, tx_after)) in lo_after.iter() {
        // Skip loopback / virtual interfaces that aren't useful for
        // operator dashboards. `docker0` / `veth*` / `br-*` pollute
        // graphs if you're running containers on the box.
        if iface == "lo"
            || iface.starts_with("veth")
            || iface.starts_with("br-")
            || iface == "docker0"
        {
            continue;
        }
        let (rx_before, tx_before) = lo_before
            .get(iface)
            .copied()
            .unwrap_or((*rx_after, *tx_after));
        // `rx_bytes` is u64; subtraction wraps in the rare case of a
        // counter rollover (32-bit machines) — saturate for safety.
        let rx_delta = rx_after.saturating_sub(rx_before) as f64;
        let tx_delta = tx_after.saturating_sub(tx_before) as f64;
        let rx_bps = if elapsed_secs > 0.0 {
            rx_delta / elapsed_secs
        } else {
            0.0
        };
        let tx_bps = if elapsed_secs > 0.0 {
            tx_delta / elapsed_secs
        } else {
            0.0
        };
        out.push(NetworkStats {
            iface: iface.clone(),
            rx_bps,
            tx_bps,
            rx_bytes_total: *rx_after,
            tx_bytes_total: *tx_after,
        });
    }
    // Sort by iface name for stable ordering (deterministic UI, easier
    // to diff in logs).
    out.sort_by(|a, b| a.iface.cmp(&b.iface));
    out
}

fn index_net(dump: &str) -> std::collections::HashMap<String, (u64, u64)> {
    let mut m = std::collections::HashMap::new();
    for line in dump.lines() {
        // `/proc/net/dev` header is two lines starting with `Inter-` or
        // ` face`. Data rows start with `<whitespace><iface>:`.
        let Some((lhs, rhs)) = line.split_once(':') else {
            continue;
        };
        let iface = lhs.trim().to_string();
        if iface.is_empty() || iface.starts_with("Inter") || iface.starts_with("face") {
            continue;
        }
        let cells: Vec<&str> = rhs.split_whitespace().collect();
        // 16 columns: 8 rx + 8 tx.
        if cells.len() < 16 {
            continue;
        }
        let rx_bytes: u64 = cells[0].parse().unwrap_or(0);
        let tx_bytes: u64 = cells[8].parse().unwrap_or(0);
        m.insert(iface, (rx_bytes, tx_bytes));
    }
    m
}

/// Extract just `(index -> name)` from an nvidia-smi CSV snapshot, used
/// by the DCGM path to splice product names into rows `dcgmi dmon`
/// doesn't carry. Parallel to `parse_nvsmi` but cheap — we only need
/// the first two columns.
fn parse_nvsmi_names(csv: &str) -> std::collections::HashMap<u32, String> {
    let mut out = std::collections::HashMap::new();
    for line in csv.lines() {
        let cells: Vec<&str> = line.split(',').map(|c| c.trim()).collect();
        if cells.len() < 2 {
            continue;
        }
        let Ok(idx) = cells[0].parse::<u32>() else {
            continue;
        };
        let name = cells[1].to_string();
        if !name.is_empty() {
            out.insert(idx, name);
        }
    }
    out
}

fn parse_nvsmi(csv: &str) -> Vec<GpuStats> {
    csv.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let cells: Vec<&str> = l.split(',').map(|c| c.trim()).collect();
            if cells.len() < 8 {
                return None;
            }
            let idx: u32 = cells[0].parse().ok()?;
            let name = cells[1].to_string();
            let util = cells[2].parse::<f32>().ok();
            let mem_u = cells[3].parse::<u64>().ok();
            let mem_t = cells[4].parse::<u64>().ok();
            let temp = cells[5].parse::<f32>().ok();
            let power = cells[6].parse::<f32>().ok();
            let power_lim = cells[7].parse::<f32>().ok();
            Some(GpuStats {
                index: idx,
                name,
                util_pct: util,
                mem_used_mib: mem_u,
                mem_total_mib: mem_t,
                temp_c: temp,
                power_w: power,
                power_limit_w: power_lim,
                source: "nvidia-smi",
                mem_temp_c: None,
                ecc_dbe_total: None,
                xid_last: None,
                throttle_reasons: None,
                throttle_reason_label: None,
            })
        })
        .collect()
}

/// Cache per-host result of the DCGM health probe. DCGM is only useful
/// when the remote has BOTH the `dcgmi` binary AND a running
/// `nv-hostengine` daemon. When the daemon is down, `dcgmi dmon`
/// hangs for ~5 s waiting on a localhost connect before failing over
/// to nvidia-smi. We probe once per process-lifetime and remember the
/// answer so subsequent `server.stats` polls skip the doomed path.
/// Re-probe after 10 min so a freshly-started hostengine is picked up.
static DCGM_HEALTHY: once_cell::sync::Lazy<
    std::sync::Mutex<std::collections::HashMap<String, (bool, std::time::Instant)>>,
> = once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

const DCGM_RECHECK: std::time::Duration = std::time::Duration::from_secs(600);

fn dcgm_cached(host: &str) -> Option<bool> {
    let m = DCGM_HEALTHY.lock().ok()?;
    let (ok, when) = *m.get(host)?;
    if when.elapsed() < DCGM_RECHECK {
        Some(ok)
    } else {
        None
    }
}

fn dcgm_cache_set(host: &str, ok: bool) {
    if let Ok(mut m) = DCGM_HEALTHY.lock() {
        m.insert(host.to_string(), (ok, std::time::Instant::now()));
    }
}

async fn collect_dcgm(target: &SshTarget) -> Result<Vec<GpuStats>, SshError> {
    // Per-host cache: skip dcgmi entirely if we recently confirmed the
    // hostengine is down. Cuts the poll from ~5 s to ~0.5 s on boxes
    // that advertise DCGM but don't run `nv-hostengine`.
    if let Some(false) = dcgm_cached(&target.host) {
        return Ok(Vec::new());
    }
    // `dcgmi discovery -l -j` lists GPUs; `dcgmi dmon -c 1 -e <fields>`
    // emits a single-shot sample. We use discovery first, dmon second.
    let out = exec(
        target,
        // Wrap dcgmi in `timeout 1s` because `dcgmi dmon` blocks for
        // its internal 5-second connect-to-localhost retry when
        // `nv-hostengine` isn't running. 1 s is enough for a healthy
        // host — an unhealthy one falls through to nvidia-smi on the
        // next tick instead of stalling the whole poll.
        //
        // Field IDs (DCGM_FI_DEV_*) — verified against dmon 3.x:
        //   203 GPU_UTIL (GPUTL)      204 MEM_COPY_UTIL (MCUTL)
        //   150 GPU_TEMP (TMPTR)      140 MEMORY_TEMP (MMTMP)
        //   155 POWER_USAGE (POWER)   160 POWER_MGMT_LIMIT (PMLMT)
        //   112 THROTTLE_REASONS (DVCCTR)   230 XID_ERRORS (XIDER)
        //   310 ECC_SBE_VOL_TOTAL (ESVTL)
        //   319 ECC_DBE_VOL_TOTAL (EDVDV)
        //   250 FB_TOTAL (FBTTL, MiB)  252 FB_USED (FBUSD, MiB)
        "timeout 1s sudo -n /usr/bin/dcgmi dmon -c 1 -e 203,204,150,140,155,160,112,230,310,319,250,252 2>/dev/null",
    )
    .await?;
    if !out.ok() || out.stdout.trim().is_empty() {
        // Remember this host has broken DCGM so the next poll skips
        // the doomed call right away.
        dcgm_cache_set(&target.host, false);
        return Ok(Vec::new());
    }
    dcgm_cache_set(&target.host, true);
    // Parse dmon output by COLUMN NAME, not position. Newer DCGM
    // releases (3.x+) emit columns in a stable but different order
    // than the `-e` request, AND sometimes inject extra fields like
    // `TOTEC` that we didn't ask for. Identifying columns by header
    // name (GPUTL / POWER / TMPTR / …) is the only robust way.
    //
    // dmon output shape:
    //   #Entity   GPUTL  MCUTL  POWER  TOTEC     TMPTR  MMTMP
    //   ID                       W      mJ        C      C
    //   GPU 0     0      0      59.2   2.07e+11  24     40
    // Header row starts with `#Entity`; the unit row is the one below.
    // Data rows start with `GPU`.
    let mut col_names: Vec<String> = Vec::new();
    let mut out_vec = Vec::new();
    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            // Strip leading `#` then collect whitespace-split tokens.
            // First token is `#Entity` or `Entity`; we care about
            // columns AFTER the entity identifier (which is 2 tokens:
            // `GPU <idx>`).
            let stripped = trimmed.trim_start_matches('#').trim();
            let tokens: Vec<&str> = stripped.split_whitespace().collect();
            // Skip the first token (`Entity` / `Entity ID`) — the
            // identifier column is two data cells (`GPU` + idx).
            col_names = tokens.iter().skip(1).map(|s| s.to_string()).collect();
            continue;
        }
        if !trimmed.starts_with("GPU") {
            // Unit row or stray line.
            continue;
        }
        let cells: Vec<&str> = trimmed.split_whitespace().collect();
        // `GPU <idx> <val0> <val1> …` → values start at index 2.
        if cells.len() < 3 || cells[0] != "GPU" {
            continue;
        }
        let idx: u32 = match cells[1].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let values: Vec<&str> = cells.iter().skip(2).copied().collect();
        let get = |name: &str| -> Option<f32> {
            let pos = col_names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(name))?;
            values.get(pos)?.parse::<f32>().ok()
        };
        // Column name → DCGM field ID:
        //   GPUTL = 203 (GPU_UTIL)
        //   MCUTL = 204 (MEM_COPY_UTIL)
        //   POWER = 156 (POWER_USAGE, watts)
        //   TOTEC = 157 (TOTAL_ENERGY, mJ — unused)
        //   TMPTR = 150 (GPU_TEMP, °C)
        //   MMTMP = 140 (MEMORY_TEMP, °C — unused)
        //   PLIMIT = 140 on some older builds; on 3.x `POWER_LIMIT`
        //   comes out as `PWR` sometimes. Try both names.
        let util = get("GPUTL");
        let temp = get("TMPTR").or_else(|| get("GPUTMP"));
        let mem_temp = get("MMTMP").or_else(|| get("MEMTMP"));
        let power = get("POWER");
        let plim = get("PMLMT")
            .or_else(|| get("PLIMIT"))
            .or_else(|| get("PWR_LIM"))
            .or_else(|| get("PWR"));
        // Integer-like fields: parse through u64 first to keep precision.
        let get_u64 = |name: &str| -> Option<u64> {
            let pos = col_names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(name))?;
            let raw = values.get(pos)?;
            if raw.eq_ignore_ascii_case("N/A") {
                return None;
            }
            raw.parse::<u64>()
                .ok()
                .or_else(|| raw.parse::<f64>().ok().map(|v| v as u64))
        };
        let ecc_dbe = get_u64("EDVDV")
            .or_else(|| get_u64("ECDBE"))
            .or_else(|| get_u64("ECCDBE"))
            .or_else(|| get_u64("DBEVOL"));
        let xid = get_u64("XIDER")
            .or_else(|| get_u64("XID"))
            .map(|v| v as u32);
        let throttle = get_u64("DVCCTR")
            .or_else(|| get_u64("TTLRSN"))
            .or_else(|| get_u64("TRSN"))
            .or_else(|| get_u64("THRREAS"));
        let mem_used = get_u64("FBUSD");
        let mem_total = get_u64("FBTTL");
        let throttle_label = throttle.and_then(decode_throttle_reasons);

        if util.is_none() && temp.is_none() && power.is_none() {
            continue;
        }
        out_vec.push(GpuStats {
            index: idx,
            name: format!("GPU {idx}"),
            util_pct: util,
            mem_used_mib: mem_used,
            mem_total_mib: mem_total,
            temp_c: temp,
            power_w: power,
            power_limit_w: plim,
            source: "dcgm",
            mem_temp_c: mem_temp,
            ecc_dbe_total: ecc_dbe,
            xid_last: xid,
            throttle_reasons: throttle,
            throttle_reason_label: throttle_label,
        });
    }
    Ok(out_vec)
}

/// Decode DCGM's clock-throttle-reasons bitmask into a short
/// operator-facing label. Returns `None` for idle/benign reasons so
/// the UI can stay quiet on healthy GPUs; returns `Some("...")` only
/// when the GPU is actually being held back.
fn decode_throttle_reasons(bits: u64) -> Option<String> {
    // DCGM bits (match NVML DCGM_FI_DEV_CLOCK_THROTTLE_REASONS):
    //   0x01 GPU_IDLE (not a real throttle)
    //   0x02 CLOCKS_SETTING (user app clock — benign)
    //   0x04 SW_POWER_CAP     (⚠ power budget hit)
    //   0x08 HW_SLOWDOWN      (⚠⚠ thermal / power emergency)
    //   0x10 SYNC_BOOST
    //   0x20 SW_THERMAL_SLOWDOWN
    //   0x40 HW_THERMAL_SLOWDOWN
    //   0x80 HW_POWER_BRAKE_SLOWDOWN
    //   0x100 DISPLAY_CLOCK_SETTING
    let mut labels = Vec::new();
    if bits & 0x04 != 0 {
        labels.push("SW power cap");
    }
    if bits & 0x08 != 0 {
        labels.push("HW slowdown");
    }
    if bits & 0x20 != 0 {
        labels.push("SW thermal");
    }
    if bits & 0x40 != 0 {
        labels.push("HW thermal");
    }
    if bits & 0x80 != 0 {
        labels.push("HW power brake");
    }
    if labels.is_empty() {
        None
    } else {
        Some(labels.join(" | "))
    }
}

fn parse_ipmi_dcmi_power(txt: &str) -> Option<f32> {
    // Example line: "    Instantaneous power reading:                   123 Watts".
    // Treat a reading of 0 as "BMC advertises DCMI but doesn't fill it
    // in" — the common ASUS / SMC / Quanta failure mode. Caller then
    // falls through to the per-PSU sensor list parser.
    for line in txt.lines() {
        if let Some(rest) = line.trim().strip_prefix("Instantaneous power reading:") {
            let num = rest.split_whitespace().next()?;
            let w = num.parse::<f32>().ok()?;
            return if w > 0.0 { Some(w) } else { None };
        }
    }
    None
}

#[allow(dead_code)] // reserved for future `[server_monitor].slow_ipmi = true` opt-in.
fn parse_ipmi_sensor_psu_watts(txt: &str) -> Option<f32> {
    // `ipmitool sensor list` POUT rows look like:
    //   "PSU1 POUT         | 420.000    | Watts      | ok    | ..."
    //   "PSU2 POUT         | na         | Watts      | na    | ..."
    // Sum the numeric values — boards with two PSUs split load so a
    // single-PSU reading under-reports actual draw. `na` rows are
    // skipped; if every row is `na` we return None.
    let mut total = 0.0f32;
    let mut any_value = false;
    for line in txt.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 3 {
            continue;
        }
        let unit = cells.get(2).copied().unwrap_or("").to_ascii_lowercase();
        if unit != "watts" {
            continue;
        }
        if let Ok(w) = cells[1].parse::<f32>() {
            total += w;
            any_value = true;
        }
    }
    if any_value && total > 0.0 {
        Some(total)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sections_splits_by_delimiter() {
        let s = "===A===\naaa\n===B===\nb1\nb2\n===END===\n";
        let m = split_sections(s);
        assert_eq!(m.get("A").unwrap().trim(), "aaa");
        assert_eq!(m.get("B").unwrap().trim(), "b1\nb2");
    }

    #[test]
    fn parse_cpu_delta_computes_util() {
        let stat = "cpu 100 0 50 800 50 0 0 0 0 0\ncpu 200 0 100 900 100 0 0 0 0 0";
        let cpu = parse_cpu(stat, "1.5 2.5 3.5 1/100 1", "8").unwrap();
        assert!(cpu.util_pct > 0.0);
        assert!(cpu.util_pct < 100.0);
        assert_eq!(cpu.cores, 8);
        assert!((cpu.load_1m - 1.5).abs() < 0.01);
        assert!((cpu.load_5m - 2.5).abs() < 0.01);
    }

    #[test]
    fn parse_mem_reads_meminfo() {
        let mi = "MemTotal:       16384 kB\nMemAvailable:    8192 kB\nSwapTotal:       4096 kB\nSwapFree:        4096 kB\n";
        let m = parse_mem(mi).unwrap();
        assert_eq!(m.total_bytes, 16384 * 1024);
        assert_eq!(m.available_bytes, 8192 * 1024);
        assert_eq!(m.used_bytes, 8192 * 1024);
        assert_eq!(m.swap_used_bytes, 0);
    }

    #[test]
    fn parse_df_extracts_rows() {
        let df = "/dev/sda1 ext4 1000 500 /\n/dev/sdb2 xfs 2000 1500 /data\n";
        let d = parse_df(df);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].mount, "/");
        assert_eq!(d[0].used_bytes, 500);
        assert_eq!(d[1].mount, "/data");
    }

    #[test]
    fn parse_sensors_reads_input_temps() {
        let j = r#"{"coretemp-isa-0000":{"Core 0":{"temp1_input":41.0,"temp1_max":80.0},"Core 1":{"temp2_input":42.5}}}"#;
        let t = parse_sensors(j).unwrap();
        assert_eq!(t.len(), 2);
        let celsius: Vec<f32> = t.iter().map(|r| r.celsius).collect();
        assert!(celsius.contains(&41.0));
        assert!(celsius.contains(&42.5));
    }

    #[test]
    fn parse_nvsmi_reads_csv_row() {
        let csv = "0, NVIDIA A100-SXM4-80GB, 35, 12345, 81920, 55, 250.5, 400.0\n";
        let g = parse_nvsmi(csv);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].index, 0);
        assert_eq!(g[0].name, "NVIDIA A100-SXM4-80GB");
        assert_eq!(g[0].util_pct, Some(35.0));
        assert_eq!(g[0].mem_used_mib, Some(12345));
        assert_eq!(g[0].temp_c, Some(55.0));
        assert_eq!(g[0].power_w, Some(250.5));
        assert_eq!(g[0].source, "nvidia-smi");
    }

    #[test]
    fn parse_network_computes_bps_and_filters_virtual() {
        let before = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 100000       50    0    0    0     0          0         0   100000       50    0    0    0     0       0          0
  eth0: 1000000     400    0    0    0     0          0         0    500000     200    0    0    0     0       0          0
 docker0: 77         1    0    0    0     0          0         0       77        1    0    0    0     0       0          0
  veth1: 42          1    0    0    0     0          0         0       42        1    0    0    0     0       0          0
";
        let after = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 100000       50    0    0    0     0          0         0   100000       50    0    0    0     0       0          0
  eth0: 1200000     450    0    0    0     0          0         0    600000     220    0    0    0     0       0          0
 docker0: 77         1    0    0    0     0          0         0       77        1    0    0    0     0       0          0
  veth1: 42          1    0    0    0     0          0         0       42        1    0    0    0     0       0          0
";
        let net = parse_network(before, after, 0.2);
        // lo, docker0, veth1 filtered → only eth0.
        assert_eq!(net.len(), 1);
        assert_eq!(net[0].iface, "eth0");
        // (1_200_000 - 1_000_000) / 0.2 = 1_000_000 bps.
        assert_eq!(net[0].rx_bps, 1_000_000.0);
        // (600_000 - 500_000) / 0.2 = 500_000 bps.
        assert_eq!(net[0].tx_bps, 500_000.0);
        assert_eq!(net[0].rx_bytes_total, 1_200_000);
        assert_eq!(net[0].tx_bytes_total, 600_000);
    }

    #[test]
    fn parse_network_handles_missing_before() {
        // When an interface appears only in the `after` snapshot (e.g.
        // hot-plug NIC), delta reports 0 bps rather than panicking.
        let before = "";
        let after = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
  eth0: 500        5    0    0    0     0          0         0      500        5    0    0    0     0       0          0
";
        let net = parse_network(before, after, 0.2);
        assert_eq!(net.len(), 1);
        assert_eq!(net[0].rx_bps, 0.0);
        assert_eq!(net[0].tx_bps, 0.0);
    }

    #[test]
    fn parse_network_counter_rollback_saturates() {
        // Rare u64 wrap / counter reset on 32-bit kernels — saturate,
        // don't panic.
        let before = "  eth0: 1000 1 0 0 0 0 0 0 1000 1 0 0 0 0 0 0\n";
        let after = "  eth0:    0 0 0 0 0 0 0 0    0 0 0 0 0 0 0 0\n";
        let net = parse_network(before, after, 0.2);
        assert_eq!(net[0].rx_bps, 0.0); // saturating_sub → 0 delta
    }

    #[test]
    fn parse_dcmi_returns_value_only_when_nonzero() {
        // Real non-zero reading → propagated.
        assert_eq!(
            parse_ipmi_dcmi_power("    Instantaneous power reading:                   123 Watts"),
            Some(123.0),
        );
        // 0 Watts from a board that advertises DCMI support but doesn't
        // fill it in — caller expects None so it falls back to sensor
        // list.
        assert_eq!(
            parse_ipmi_dcmi_power("    Instantaneous power reading:                     0 Watts"),
            None,
        );
        assert_eq!(parse_ipmi_dcmi_power(""), None);
    }

    #[test]
    fn parse_sensor_psu_watts_sums_numeric_rows() {
        let txt = "PSU1 POUT         | 420.500    | Watts      | ok    | na        | na\n\
                   PSU2 POUT         | na         | Watts      | na    | na        | na";
        assert_eq!(parse_ipmi_sensor_psu_watts(txt), Some(420.5));
        let two =
            "PSU1 POUT | 200.0 | Watts | ok | na | na\nPSU2 POUT | 210.0 | Watts | ok | na | na";
        assert_eq!(parse_ipmi_sensor_psu_watts(two), Some(410.0));
        // All na → None so UI hides the line.
        let all_na = "PSU1 POUT | na | Watts | na | na | na\nPSU2 POUT | na | Watts | na | na | na";
        assert_eq!(parse_ipmi_sensor_psu_watts(all_na), None);
    }
}
