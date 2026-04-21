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
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerStats {
    /// IPMI DCMI reading if the BMC exposes it.
    pub psu_watts: Option<f32>,
    /// Sum of per-GPU power — useful even without IPMI.
    pub gpu_watts: Option<f32>,
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
        echo '===STAT==='
        head -1 /proc/stat 2>/dev/null
        sleep 0.2
        head -1 /proc/stat 2>/dev/null
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
    stats.cpu = parse_cpu(
        sections.get("STAT").map(|s| s.as_str()).unwrap_or(""),
        sections.get("LOAD").map(|s| s.as_str()).unwrap_or(""),
        sections.get("CPUINFO").map(|s| s.as_str()).unwrap_or(""),
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
    let psu_watts = parse_ipmi_dcmi_power(
        sections.get("IPMIPWR").map(|s| s.as_str()).unwrap_or(""),
    );
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
        if let Some(tag) = line
            .strip_prefix("===")
            .and_then(|s| s.strip_suffix("==="))
        {
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
    let dt = t1.checked_sub(t0).unwrap_or(0);
    let di = i1.checked_sub(i0).unwrap_or(0);
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
            })
        })
        .collect()
}

async fn collect_dcgm(target: &SshTarget) -> Result<Vec<GpuStats>, SshError> {
    // `dcgmi discovery -l -j` lists GPUs; `dcgmi dmon -c 1 -e <fields>`
    // emits a single-shot sample. We use discovery first, dmon second.
    let out = exec(
        target,
        "sudo -n /usr/bin/dcgmi dmon -c 1 -e 203,204,155,156,150,140 2>/dev/null",
    )
    .await?;
    if !out.ok() || out.stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    // dmon text output is whitespace-aligned with a header line. Skip
    // lines starting with `#` / `GPU` / empty. Column order from -e:
    //   203 = DCGM_FI_DEV_GPU_UTIL
    //   204 = DCGM_FI_DEV_MEM_COPY_UTIL  (unused here but keeps pairing)
    //   155 = DCGM_FI_DEV_GPU_TEMP
    //   156 = DCGM_FI_DEV_POWER_USAGE
    //   150 = DCGM_FI_DEV_MEM_CLOCK      (unused)
    //   140 = DCGM_FI_DEV_POWER_MGMT_LIMIT
    let mut out_vec = Vec::new();
    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("GPU") {
            continue;
        }
        let cells: Vec<&str> = trimmed.split_whitespace().collect();
        // Row shape: `GPU 0    <util> <memcopy> <temp> <power> <memclk> <plimit>`
        if cells.len() < 8 || cells[0] != "GPU" {
            continue;
        }
        let idx: u32 = match cells[1].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let util = cells[2].parse::<f32>().ok();
        let temp = cells[4].parse::<f32>().ok();
        let power = cells[5].parse::<f32>().ok();
        let plim = cells[7].parse::<f32>().ok();
        out_vec.push(GpuStats {
            index: idx,
            name: format!("GPU {idx}"), // dmon doesn't carry the name; UI can merge with nvsmi for the model string if needed
            util_pct: util,
            mem_used_mib: None,
            mem_total_mib: None,
            temp_c: temp,
            power_w: power,
            power_limit_w: plim,
            source: "dcgm",
        });
    }
    Ok(out_vec)
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
        let two = "PSU1 POUT | 200.0 | Watts | ok | na | na\nPSU2 POUT | 210.0 | Watts | ok | na | na";
        assert_eq!(parse_ipmi_sensor_psu_watts(two), Some(410.0));
        // All na → None so UI hides the line.
        let all_na = "PSU1 POUT | na | Watts | na | na | na\nPSU2 POUT | na | Watts | na | na | na";
        assert_eq!(parse_ipmi_sensor_psu_watts(all_na), None);
    }
}
