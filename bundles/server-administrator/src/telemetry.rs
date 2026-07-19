use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Map, Value};

const MAX_INVENTORY_BYTES: usize = 32 * 1024;
const MAX_TELEMETRY_BYTES: usize = 128 * 1024;
const MAX_GPUS: usize = 16;
const MAX_DISKS: usize = 32;
const MAX_TEMPERATURES: usize = 64;
const MAX_INTERFACES: usize = 32;

pub(crate) struct MetricSample {
    pub metric: String,
    pub value: f64,
    pub unit: &'static str,
    pub labels: Value,
}

pub(crate) fn parse_inventory(stdout: &str) -> Result<Value, &'static str> {
    const FIELDS: [&str; 12] = [
        "hostname",
        "kernel",
        "architecture",
        "machine_id",
        "boot_id",
        "cpu_model",
        "logical_cpus",
        "memory_kib",
        "os_pretty_name",
        "uptime_seconds",
        "dmi_uuid",
        "dmi_serial",
    ];
    if stdout.len() > MAX_INVENTORY_BYTES {
        return Err("inventory output exceeded the Bundle parser ceiling");
    }
    let mut facts = BTreeMap::new();
    let mut gpus = Vec::new();
    for line in stdout.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            return Err("inventory output contained a malformed line");
        };
        if raw.len() > 2_048 || raw.chars().any(char::is_control) {
            return Err("inventory output violated its signed field contract");
        }
        if key == "gpu" {
            if gpus.len() >= MAX_GPUS {
                return Err("inventory output exceeded the GPU ceiling");
            }
            gpus.push(parse_inventory_gpu(raw)?);
            continue;
        }
        if !FIELDS.contains(&key) || facts.insert(key, raw).is_some() {
            return Err("inventory output violated its signed field contract");
        }
    }
    if facts.len() != FIELDS.len() || facts["hostname"].is_empty() {
        return Err("inventory output omitted required signed fields");
    }
    let logical_cpus = parse_u64(facts["logical_cpus"])?;
    let memory_kib = parse_u64(facts["memory_kib"])?;
    let uptime_seconds = parse_u64(facts["uptime_seconds"])?;
    Ok(json!({
        "hostname": facts["hostname"],
        "kernel": facts["kernel"],
        "architecture": facts["architecture"],
        "machine_id": optional_text(facts["machine_id"]),
        "boot_id": optional_text(facts["boot_id"]),
        "cpu_model": optional_text(facts["cpu_model"]),
        "logical_cpus": logical_cpus,
        "memory_kib": memory_kib,
        "memory_total_bytes": memory_kib.saturating_mul(1024),
        "os_pretty_name": optional_text(facts["os_pretty_name"]),
        "uptime_seconds": uptime_seconds,
        "dmi_uuid": optional_text(facts["dmi_uuid"]),
        "dmi_serial": optional_text(facts["dmi_serial"]),
        "gpu_count": gpus.len(),
        "gpus": gpus,
    }))
}

fn parse_inventory_gpu(raw: &str) -> Result<Value, &'static str> {
    let mut fields = raw.splitn(3, ',').map(str::trim);
    let index = fields
        .next()
        .ok_or("inventory GPU row is malformed")?
        .parse::<u32>()
        .map_err(|_| "inventory GPU index is invalid")?;
    let uuid = fields.next().ok_or("inventory GPU row is malformed")?;
    let name = fields.next().ok_or("inventory GPU row is malformed")?;
    if index > 255 || uuid.len() > 128 || name.is_empty() || name.len() > 256 {
        return Err("inventory GPU row violated its bounded field contract");
    }
    Ok(json!({"index": index, "uuid": uuid, "name": name}))
}

pub(crate) fn parse_telemetry(stdout: &str) -> Result<Value, &'static str> {
    if stdout.len() > MAX_TELEMETRY_BYTES {
        return Err("telemetry output exceeded the Bundle parser ceiling");
    }
    let sections = split_sections(stdout)?;
    let hostname = required_section(&sections, "HOST")?.trim();
    if hostname.is_empty() || hostname.len() > 253 || hostname.chars().any(char::is_control) {
        return Err("telemetry hostname is invalid");
    }
    let uptime_seconds = required_section(&sections, "UPTIME")?
        .trim()
        .parse::<u64>()
        .map_err(|_| "telemetry uptime is invalid")?;
    let cpu = parse_cpu(
        required_section(&sections, "STAT0")?,
        required_section(&sections, "STAT1")?,
        required_section(&sections, "LOAD")?,
        required_section(&sections, "CPUINFO")?,
    )?;
    let memory = parse_memory(required_section(&sections, "MEM")?)?;
    let disks = parse_disks(required_section(&sections, "DF")?)?;
    let mut collector_warnings = Vec::new();
    let temperatures =
        match parse_temperatures(sections.get("SENSORS").map_or("{}", String::as_str)) {
            Ok(readings) => readings,
            Err(message) => {
                collector_warnings.push(message.to_string());
                Vec::new()
            }
        };
    let network = parse_network(
        required_section(&sections, "NET0")?,
        required_section(&sections, "NET1")?,
    )?;
    let mut gpus = match parse_nvidia_smi(sections.get("NVSMI").map_or("", String::as_str)) {
        Ok(gpus) => gpus,
        Err(message) => {
            collector_warnings.push(message.to_string());
            Vec::new()
        }
    };
    merge_nvidia_health(
        &mut gpus,
        sections.get("NVSMI_HEALTH").map_or("", String::as_str),
    )?;
    merge_dcgm(&mut gpus, sections.get("DCGM").map_or("", String::as_str))?;
    let xid_events = parse_xid_events(sections.get("XID").map_or("", String::as_str));
    let psu_watts = parse_ipmi_power(sections.get("IPMI").map_or("", String::as_str));
    let gpu_watts = finite_values(&gpus, "power_w").sum::<f64>();
    let availability = parse_availability(sections.get("AVAILABILITY").map_or("", String::as_str))?;
    collector_warnings.extend(availability_warnings(
        &availability,
        &gpus,
        &temperatures,
        psu_watts,
    ));
    let mut stats = json!({
        "hostname": hostname,
        "uptime_seconds": uptime_seconds,
        "cpu": cpu,
        "memory": memory,
        "disks": disks,
        "temperatures": temperatures,
        "gpus": gpus,
        "power": {
            "psu_watts": psu_watts,
            "gpu_watts": if gpu_watts > 0.0 { Some(gpu_watts) } else { None },
        },
        "network": network,
        "xid_events": xid_events,
        "availability": availability,
        "warnings": collector_warnings,
    });
    let summary = telemetry_summary(&stats);
    stats
        .as_object_mut()
        .expect("telemetry snapshot is constructed as an object")
        .insert("summary".into(), summary);
    Ok(stats)
}

fn split_sections(stdout: &str) -> Result<BTreeMap<String, String>, &'static str> {
    const ALLOWED: [&str; 16] = [
        "HOST",
        "UPTIME",
        "LOAD",
        "CPUINFO",
        "STAT0",
        "STAT1",
        "NET0",
        "NET1",
        "MEM",
        "DF",
        "SENSORS",
        "NVSMI",
        "NVSMI_HEALTH",
        "DCGM",
        "IPMI",
        "XID",
    ];
    let mut sections = BTreeMap::new();
    let mut current: Option<&str> = None;
    let mut buffer = String::new();
    let mut finish = |name: Option<&str>, body: &mut String| -> Result<(), &'static str> {
        if let Some(name) = name {
            if sections
                .insert(name.to_string(), std::mem::take(body))
                .is_some()
            {
                return Err("telemetry output repeated a signed section");
            }
        }
        Ok(())
    };
    for line in stdout.lines() {
        if let Some(tag) = line
            .strip_prefix("===")
            .and_then(|value| value.strip_suffix("==="))
        {
            finish(current.take(), &mut buffer)?;
            if tag == "END" {
                current = None;
            } else if tag == "AVAILABILITY" || ALLOWED.contains(&tag) {
                current = Some(tag);
            } else {
                return Err("telemetry output contained an unknown section");
            }
        } else if current.is_some() {
            buffer.push_str(line);
            buffer.push('\n');
        } else if !line.trim().is_empty() {
            return Err("telemetry output escaped its signed sections");
        }
    }
    finish(current, &mut buffer)?;
    Ok(sections)
}

fn required_section<'a>(
    sections: &'a BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, &'static str> {
    sections
        .get(key)
        .map(String::as_str)
        .ok_or("telemetry output omitted a required signed section")
}

fn parse_cpu(stat0: &str, stat1: &str, load: &str, cores: &str) -> Result<Value, &'static str> {
    let (total0, idle0) = parse_cpu_stat(stat0)?;
    let (total1, idle1) = parse_cpu_stat(stat1)?;
    let total_delta = total1.saturating_sub(total0);
    let idle_delta = idle1.saturating_sub(idle0);
    let util_percent = if total_delta == 0 {
        0.0
    } else {
        (total_delta.saturating_sub(idle_delta) as f64 * 100.0) / total_delta as f64
    };
    let mut loads = load.split_whitespace();
    let load_1m = parse_f64(loads.next().unwrap_or(""))?;
    let load_5m = parse_f64(loads.next().unwrap_or(""))?;
    let logical_cpus = cores
        .trim()
        .parse::<u32>()
        .map_err(|_| "telemetry CPU count is invalid")?;
    Ok(json!({
        "util_percent": util_percent,
        "load_1m": load_1m,
        "load_5m": load_5m,
        "logical_cpus": logical_cpus,
    }))
}

fn parse_cpu_stat(section: &str) -> Result<(u64, u64), &'static str> {
    let mut fields = section.split_whitespace();
    if fields.next() != Some("cpu") {
        return Err("telemetry CPU snapshot is malformed");
    }
    let values: Vec<u64> = fields
        .take(10)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| "telemetry CPU counter is invalid")
        })
        .collect::<Result<_, _>>()?;
    if values.len() < 4 {
        return Err("telemetry CPU snapshot is incomplete");
    }
    let idle = values[3].saturating_add(*values.get(4).unwrap_or(&0));
    Ok((values.iter().copied().sum(), idle))
}

fn parse_memory(section: &str) -> Result<Value, &'static str> {
    let mut values = HashMap::new();
    for line in section.lines().take(256) {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let value = rest
            .split_whitespace()
            .next()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0)
            .saturating_mul(1024);
        values.insert(key, value);
    }
    let total = *values
        .get("MemTotal")
        .ok_or("telemetry memory total is missing")?;
    let available = *values
        .get("MemAvailable")
        .or_else(|| values.get("MemFree"))
        .ok_or("telemetry available memory is missing")?;
    if total == 0 || available > total {
        return Err("telemetry memory counters are inconsistent");
    }
    let used = total - available;
    let swap_total = *values.get("SwapTotal").unwrap_or(&0);
    let swap_free = *values.get("SwapFree").unwrap_or(&0);
    Ok(json!({
        "total_bytes": total,
        "used_bytes": used,
        "available_bytes": available,
        "used_percent": used as f64 * 100.0 / total as f64,
        "swap_total_bytes": swap_total,
        "swap_used_bytes": swap_total.saturating_sub(swap_free),
    }))
}

fn parse_disks(section: &str) -> Result<Vec<Value>, &'static str> {
    let mut disks = Vec::new();
    for line in section.lines().filter(|line| !line.trim().is_empty()) {
        if disks.len() >= MAX_DISKS {
            return Err("telemetry disk list exceeded its ceiling");
        }
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() != 6 || fields.iter().any(|field| field.len() > 512) {
            return Err("telemetry disk row is malformed");
        }
        let total_kib = parse_u64(fields[2])?;
        let used_kib = parse_u64(fields[3])?;
        let used_percent = fields[4]
            .trim_end_matches('%')
            .parse::<f64>()
            .map_err(|_| "telemetry disk percentage is invalid")?;
        disks.push(json!({
            "source": fields[0],
            "filesystem": fields[1],
            "total_bytes": total_kib.saturating_mul(1024),
            "used_bytes": used_kib.saturating_mul(1024),
            "used_percent": used_percent,
            "mount": fields[5],
        }));
    }
    Ok(disks)
}

fn parse_temperatures(section: &str) -> Result<Vec<Value>, &'static str> {
    if section.trim().is_empty() || section.trim() == "{}" {
        return Ok(Vec::new());
    }
    let value: Value =
        serde_json::from_str(section).map_err(|_| "telemetry sensor JSON is invalid")?;
    let mut readings = Vec::new();
    collect_temperatures(&value, &mut Vec::new(), &mut readings)?;
    Ok(readings)
}

fn collect_temperatures(
    value: &Value,
    path: &mut Vec<String>,
    output: &mut Vec<Value>,
) -> Result<(), &'static str> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for (key, child) in object {
        if is_temperature_input(key) {
            if let Some(celsius) = child.as_f64() {
                if !(-100.0..=250.0).contains(&celsius) {
                    return Err("telemetry temperature is outside its bounded domain");
                }
                if output.len() >= MAX_TEMPERATURES {
                    return Err("telemetry temperature list exceeded its ceiling");
                }
                output.push(json!({
                    "chip": path.first().cloned().unwrap_or_else(|| "sensor".into()),
                    "label": path.last().cloned().unwrap_or_else(|| key.clone()),
                    "celsius": celsius,
                }));
            }
        } else {
            path.push(key.clone());
            collect_temperatures(child, path, output)?;
            path.pop();
        }
    }
    Ok(())
}

fn is_temperature_input(key: &str) -> bool {
    key.strip_prefix("temp")
        .and_then(|suffix| suffix.strip_suffix("_input"))
        .is_some_and(|index| !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
}

fn parse_network(before: &str, after: &str) -> Result<Vec<Value>, &'static str> {
    let before = network_counters(before)?;
    let after = network_counters(after)?;
    let mut rows = Vec::new();
    for (iface, (rx_after, tx_after)) in after {
        if virtual_interface(&iface) {
            continue;
        }
        if rows.len() >= MAX_INTERFACES {
            return Err("telemetry network list exceeded its ceiling");
        }
        let (rx_before, tx_before) = before.get(&iface).copied().unwrap_or((rx_after, tx_after));
        rows.push(json!({
            "interface": iface,
            "rx_bps": rx_after.saturating_sub(rx_before) as f64 / 0.2,
            "tx_bps": tx_after.saturating_sub(tx_before) as f64 / 0.2,
            "rx_bytes_total": rx_after,
            "tx_bytes_total": tx_after,
        }));
    }
    Ok(rows)
}

fn network_counters(section: &str) -> Result<BTreeMap<String, (u64, u64)>, &'static str> {
    let mut counters = BTreeMap::new();
    for line in section.lines().skip(2) {
        let Some((iface, raw)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<&str> = raw.split_whitespace().collect();
        if fields.len() < 16 {
            continue;
        }
        let name = iface.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("telemetry interface name is invalid");
        }
        let rx = parse_u64(fields[0])?;
        let tx = parse_u64(fields[8])?;
        counters.insert(name.to_string(), (rx, tx));
    }
    Ok(counters)
}

fn virtual_interface(name: &str) -> bool {
    name == "lo"
        || [
            "veth", "docker", "br-", "virbr", "tun", "tap", "cni", "flannel",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn parse_nvidia_smi(section: &str) -> Result<Vec<Value>, &'static str> {
    let mut gpus = Vec::new();
    for line in section.lines().filter(|line| !line.trim().is_empty()) {
        if gpus.len() >= MAX_GPUS {
            return Err("telemetry GPU list exceeded its ceiling");
        }
        let cells: Vec<&str> = line.split(',').map(str::trim).collect();
        if cells.len() != 8 {
            return Err("telemetry nvidia-smi row is malformed");
        }
        let index = cells[0]
            .parse::<u32>()
            .map_err(|_| "telemetry GPU index is invalid")?;
        let numeric = |raw: &str| -> Option<f64> {
            raw.parse::<f64>().ok().filter(|value| value.is_finite())
        };
        gpus.push(json!({
            "index": index,
            "name": cells[1],
            "source": "nvidia-smi",
            "util_percent": numeric(cells[2]),
            "memory_used_mib": numeric(cells[3]),
            "memory_total_mib": numeric(cells[4]),
            "temperature_c": numeric(cells[5]),
            "power_w": numeric(cells[6]),
            "power_limit_w": numeric(cells[7]),
            "memory_temperature_c": null,
            "ecc_dbe_total": null,
            "xid_last": null,
            "throttle_reasons": null,
            "throttle_reason_label": null,
        }));
    }
    Ok(gpus)
}

fn merge_nvidia_health(gpus: &mut [Value], section: &str) -> Result<(), &'static str> {
    for line in section.lines().filter(|line| !line.trim().is_empty()) {
        let cells: Vec<&str> = line.split(',').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let Ok(index) = cells[0].parse::<u32>() else {
            continue;
        };
        let Some(gpu) = gpus
            .iter_mut()
            .find(|gpu| gpu.get("index").and_then(Value::as_u64) == Some(u64::from(index)))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        gpu.insert("uuid".into(), json!(cells[1]));
        gpu.insert(
            "ecc_dbe_total".into(),
            cells[2].parse::<u64>().map_or(Value::Null, Value::from),
        );
        let throttle = cells[3..].join(", ");
        if let Some(bits) = throttle
            .strip_prefix("0x")
            .and_then(|raw| u64::from_str_radix(raw, 16).ok())
        {
            gpu.insert("throttle_reasons".into(), json!(bits));
            if let Some(label) = decode_throttle_reasons(bits) {
                gpu.insert("throttle_reason_label".into(), json!(label));
            }
        } else if !throttle.is_empty() && throttle != "None" && throttle != "N/A" {
            gpu.insert("throttle_reason_label".into(), json!(throttle));
        }
    }
    Ok(())
}

fn decode_throttle_reasons(bits: u64) -> Option<String> {
    let mut labels = Vec::new();
    for (mask, label) in [
        (0x04, "SW power cap"),
        (0x08, "HW slowdown"),
        (0x20, "SW thermal"),
        (0x40, "HW thermal"),
        (0x80, "HW power brake"),
    ] {
        if bits & mask != 0 {
            labels.push(label);
        }
    }
    (!labels.is_empty()).then(|| labels.join(" | "))
}

fn merge_dcgm(gpus: &mut [Value], section: &str) -> Result<(), &'static str> {
    let mut columns = Vec::new();
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            columns = trimmed
                .trim_start_matches('#')
                .split_whitespace()
                .skip(1)
                .map(str::to_string)
                .collect();
            continue;
        }
        if !trimmed.starts_with("GPU ") || columns.is_empty() {
            continue;
        }
        let cells: Vec<&str> = trimmed.split_whitespace().collect();
        let Some(index) = cells.get(1).and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let values = &cells[2..];
        let value = |names: &[&str]| -> Option<f64> {
            names.iter().find_map(|name| {
                let position = columns
                    .iter()
                    .position(|column| column.eq_ignore_ascii_case(name))?;
                values.get(position)?.parse::<f64>().ok()
            })
        };
        let Some(gpu) = gpus
            .iter_mut()
            .find(|gpu| gpu.get("index").and_then(Value::as_u64) == Some(u64::from(index)))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (field, names) in [
            ("util_percent", ["GPUTL", "GPUUT"]),
            ("temperature_c", ["TMPTR", "GPUTMP"]),
            ("memory_temperature_c", ["MMTMP", "MEMTMP"]),
            ("power_w", ["POWER", "PWR"]),
            ("power_limit_w", ["PMLMT", "PLIMIT"]),
        ] {
            if let Some(number) = value(&names) {
                gpu.insert(field.into(), json!(number));
                gpu.insert("source".into(), json!("dcgm"));
            }
        }
        for (field, names) in [
            ("ecc_dbe_total", ["EDVDV", "ECCDBE"]),
            ("xid_last", ["XIDER", "XID"]),
            ("throttle_reasons", ["DVCCTR", "TTLRSN"]),
        ] {
            if let Some(number) = value(&names) {
                gpu.insert(field.into(), json!(number as u64));
                gpu.insert("source".into(), json!("dcgm"));
            }
        }
    }
    Ok(())
}

fn parse_xid_events(section: &str) -> Vec<Value> {
    section
        .lines()
        .rev()
        .take(16)
        .filter_map(|line| {
            let marker = line.find("): ")? + 3;
            let code = line[marker..]
                .split(|character: char| !character.is_ascii_digit())
                .next()?
                .parse::<u32>()
                .ok()?;
            Some(json!({"code": code, "excerpt": line.chars().take(512).collect::<String>()}))
        })
        .collect()
}

fn parse_ipmi_power(section: &str) -> Option<f64> {
    section.lines().find_map(|line| {
        if !line.to_ascii_lowercase().contains("instantaneous") {
            return None;
        }
        line.split_whitespace()
            .find_map(|field| field.parse::<f64>().ok().filter(|value| *value > 0.0))
    })
}

fn parse_availability(section: &str) -> Result<Value, &'static str> {
    let mut values = Map::new();
    for line in section.lines().filter(|line| !line.trim().is_empty()) {
        let Some((key, raw)) = line.split_once('=') else {
            return Err("telemetry availability row is malformed");
        };
        if !["sensors", "nvidia_smi", "dcgm", "ipmitool"].contains(&key) || values.contains_key(key)
        {
            return Err("telemetry availability violated its signed field contract");
        }
        values.insert(key.into(), json!(raw == "1"));
    }
    for key in ["sensors", "nvidia_smi", "dcgm", "ipmitool"] {
        values.entry(key).or_insert(Value::Bool(false));
    }
    Ok(Value::Object(values))
}

fn availability_warnings(
    availability: &Value,
    gpus: &[Value],
    temperatures: &[Value],
    psu_watts: Option<f64>,
) -> Vec<String> {
    let available = |key: &str| {
        availability
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut warnings = Vec::new();
    if !available("sensors") {
        warnings.push("lm-sensors is not available".into());
    } else if temperatures.is_empty() {
        warnings.push("lm-sensors returned no temperature inputs".into());
    }
    if available("nvidia_smi") && gpus.is_empty() {
        warnings.push("nvidia-smi returned no parseable GPU rows".into());
    }
    if available("dcgm")
        && !gpus.is_empty()
        && !gpus
            .iter()
            .any(|gpu| gpu.get("source").and_then(Value::as_str) == Some("dcgm"))
    {
        warnings.push("DCGM returned no usable health row; nvidia-smi fallback is active".into());
    }
    if available("ipmitool") && psu_watts.is_none() {
        warnings.push("IPMI DCMI did not expose instantaneous PSU power".into());
    }
    warnings
}

fn telemetry_summary(stats: &Value) -> Value {
    let cpu = stats.get("cpu").unwrap_or(&Value::Null);
    let memory = stats.get("memory").unwrap_or(&Value::Null);
    let disks = array_at(stats, "disks");
    let temperatures = array_at(stats, "temperatures");
    let gpus = array_at(stats, "gpus");
    let network = array_at(stats, "network");
    let psu_watts = stats
        .get("power")
        .and_then(|power| power.get("psu_watts"))
        .and_then(Value::as_f64);
    let gpu_watts = stats
        .get("power")
        .and_then(|power| power.get("gpu_watts"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    json!({
        "cpu_util_percent": number_at(cpu, "util_percent"),
        "memory_used_percent": number_at(memory, "used_percent"),
        "disk_max_used_percent": finite_values(disks, "used_percent").reduce(f64::max),
        "hottest_temperature_c": finite_values(temperatures, "celsius").reduce(f64::max),
        "gpu_count": gpus.len(),
        "gpu_average_util_percent": average(finite_values(gpus, "util_percent")),
        "gpu_max_temperature_c": finite_values(gpus, "temperature_c").reduce(f64::max),
        "gpu_total_power_w": if gpu_watts > 0.0 { Some(gpu_watts) } else { None },
        "psu_watts": psu_watts,
        "network_rx_bps": finite_values(network, "rx_bps").sum::<f64>(),
        "network_tx_bps": finite_values(network, "tx_bps").sum::<f64>(),
    })
}

pub(crate) fn metric_samples(stats: &Value) -> Vec<MetricSample> {
    let mut samples = Vec::new();
    push_object_metric(
        &mut samples,
        stats,
        "cpu",
        "util_percent",
        "cpu.util",
        "percent",
        json!({}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "cpu",
        "load_1m",
        "cpu.load_1m",
        "load",
        json!({}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "cpu",
        "load_5m",
        "cpu.load_5m",
        "load",
        json!({}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "memory",
        "used_percent",
        "mem.used_percent",
        "percent",
        json!({}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "memory",
        "used_bytes",
        "mem.used_bytes",
        "bytes",
        json!({}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "memory",
        "available_bytes",
        "mem.available_bytes",
        "bytes",
        json!({}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "memory",
        "swap_used_bytes",
        "mem.swap_used_bytes",
        "bytes",
        json!({}),
    );
    if let Some(value) = stats.get("uptime_seconds").and_then(Value::as_f64) {
        push_metric(&mut samples, "uptime_seconds", value, "seconds", json!({}));
    }
    for disk in array_at(stats, "disks") {
        let mount = disk
            .get("mount")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        for (field, suffix, unit) in [
            ("used_bytes", "used_bytes", "bytes"),
            ("total_bytes", "total_bytes", "bytes"),
            ("used_percent", "used_percent", "percent"),
        ] {
            if let Some(value) = disk.get(field).and_then(Value::as_f64) {
                push_metric(
                    &mut samples,
                    format!("disk.{suffix}"),
                    value,
                    unit,
                    json!({"mount": mount}),
                );
            }
        }
    }
    for temperature in array_at(stats, "temperatures") {
        let chip = temperature
            .get("chip")
            .and_then(Value::as_str)
            .unwrap_or("sensor");
        let label = temperature
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("input");
        if let Some(value) = temperature.get("celsius").and_then(Value::as_f64) {
            push_metric(
                &mut samples,
                "temp.celsius",
                value,
                "celsius",
                json!({"chip": chip, "source": label}),
            );
        }
    }
    for gpu in array_at(stats, "gpus") {
        let index = gpu.get("index").and_then(Value::as_u64).unwrap_or(0);
        let labels = json!({
            "gpu_index": index,
            "gpu_name": gpu.get("name").and_then(Value::as_str).unwrap_or("GPU"),
            "source": gpu.get("source").and_then(Value::as_str).unwrap_or("unknown"),
        });
        for (field, suffix, unit) in [
            ("util_percent", "util", "percent"),
            ("memory_used_mib", "mem_used_mib", "mib"),
            ("memory_total_mib", "mem_total_mib", "mib"),
            ("temperature_c", "temp", "celsius"),
            ("memory_temperature_c", "mem_temp", "celsius"),
            ("power_w", "power_w", "watts"),
            ("power_limit_w", "power_limit_w", "watts"),
            ("ecc_dbe_total", "ecc_dbe", "count"),
            ("xid_last", "xid", "code"),
            ("throttle_reasons", "throttle_bits", "bitmask"),
        ] {
            if let Some(value) = gpu.get(field).and_then(Value::as_f64) {
                let mut metric_labels = labels.clone();
                if suffix == "power_w" {
                    if let Some(limit) = gpu.get("power_limit_w").and_then(Value::as_f64) {
                        metric_labels["power_limit_w"] = json!(limit);
                    }
                }
                push_metric(
                    &mut samples,
                    format!("gpu.{index}.{suffix}"),
                    value,
                    unit,
                    metric_labels,
                );
            }
        }
        if let (Some(used), Some(total)) = (
            gpu.get("memory_used_mib").and_then(Value::as_f64),
            gpu.get("memory_total_mib").and_then(Value::as_f64),
        ) {
            if total > 0.0 {
                push_metric(
                    &mut samples,
                    format!("gpu.{index}.mem_used_percent"),
                    used * 100.0 / total,
                    "percent",
                    labels.clone(),
                );
            }
        }
    }
    if let Some(code) = array_at(stats, "xid_events")
        .first()
        .and_then(|event| event.get("code"))
        .and_then(Value::as_f64)
    {
        push_metric(
            &mut samples,
            "gpu.xid_last",
            code,
            "code",
            json!({"source": "dmesg"}),
        );
    }
    for interface in array_at(stats, "network") {
        let name = interface
            .get("interface")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        for (field, suffix, unit) in [
            ("rx_bps", "rx_bps", "bytes_per_sec"),
            ("tx_bps", "tx_bps", "bytes_per_sec"),
            ("rx_bytes_total", "rx_bytes_total", "bytes"),
            ("tx_bytes_total", "tx_bytes_total", "bytes"),
        ] {
            if let Some(value) = interface.get(field).and_then(Value::as_f64) {
                push_metric(
                    &mut samples,
                    format!("nic.{name}.{suffix}"),
                    value,
                    unit,
                    json!({"iface_kind": "physical"}),
                );
            }
        }
    }
    push_object_metric(
        &mut samples,
        stats,
        "power",
        "psu_watts",
        "power.psu_watts",
        "watts",
        json!({"source":"ipmitool"}),
    );
    push_object_metric(
        &mut samples,
        stats,
        "power",
        "gpu_watts",
        "power.gpu_watts",
        "watts",
        json!({"source":"gpu"}),
    );
    samples
}

fn push_object_metric(
    output: &mut Vec<MetricSample>,
    root: &Value,
    object: &str,
    field: &str,
    metric: &str,
    unit: &'static str,
    labels: Value,
) {
    if let Some(value) = root
        .get(object)
        .and_then(|value| value.get(field))
        .and_then(Value::as_f64)
    {
        push_metric(output, metric, value, unit, labels);
    }
}

fn push_metric(
    output: &mut Vec<MetricSample>,
    metric: impl Into<String>,
    value: f64,
    unit: &'static str,
    labels: Value,
) {
    if value.is_finite() {
        output.push(MetricSample {
            metric: metric.into(),
            value,
            unit,
            labels,
        });
    }
}

fn array_at<'a>(root: &'a Value, key: &str) -> &'a [Value] {
    root.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn number_at(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn finite_values<'a>(values: &'a [Value], key: &'a str) -> impl Iterator<Item = f64> + 'a {
    values
        .iter()
        .filter_map(move |value| value.get(key).and_then(Value::as_f64))
        .filter(|value| value.is_finite())
}

fn average(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0_u64), |(sum, count), value| (sum + value, count + 1));
    (count > 0).then_some(sum / count as f64)
}

fn optional_text(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn parse_u64(raw: &str) -> Result<u64, &'static str> {
    raw.parse::<u64>()
        .map_err(|_| "telemetry integer value is invalid")
}

fn parse_f64(raw: &str) -> Result<f64, &'static str> {
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or("telemetry numeric value is invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_preserves_machine_dmi_and_gpu_identity() {
        let inventory = parse_inventory(concat!(
            "hostname=edge-one\n",
            "kernel=Linux 6.8\n",
            "architecture=x86_64\n",
            "machine_id=machine-one\n",
            "boot_id=boot-one\n",
            "cpu_model=Fixture CPU\n",
            "logical_cpus=16\n",
            "memory_kib=32768000\n",
            "os_pretty_name=Fixture Linux\n",
            "uptime_seconds=42\n",
            "dmi_uuid=dmi-one\n",
            "dmi_serial=serial-one\n",
            "gpu=0,GPU-fixture-uuid,NVIDIA Fixture\n",
        ))
        .unwrap();
        assert_eq!(inventory["logical_cpus"], 16);
        assert_eq!(inventory["dmi_uuid"], "dmi-one");
        assert_eq!(inventory["gpus"][0]["name"], "NVIDIA Fixture");
    }

    #[test]
    fn rich_telemetry_is_bounded_and_flattens_numeric_metrics() {
        let fixture = concat!(
            "===HOST===\nedge-one\n",
            "===UPTIME===\n42\n",
            "===LOAD===\n0.25 0.50 0.75 1/10 1\n",
            "===CPUINFO===\n8\n",
            "===STAT0===\ncpu 100 0 100 800 0 0 0 0\n",
            "===NET0===\nInter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n eth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n",
            "===STAT1===\ncpu 150 0 150 900 0 0 0 0\n",
            "===NET1===\nInter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n eth0: 1200 0 0 0 0 0 0 0 2600 0 0 0 0 0 0 0\n",
            "===MEM===\nMemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 100 kB\nSwapFree: 60 kB\n",
            "===DF===\n/dev/root|ext4|1000|500|50%|/\n",
            "===SENSORS===\n{\"coretemp\":{\"Package\":{\"temp1_input\":55.0},\"Voltage\":{\"in0_input\":0.656},\"Fan\":{\"fan1_input\":1200}}}\n",
            "===NVSMI===\n0, Fixture GPU, 70, 100, 200, 60, 120, 250\n",
            "===NVSMI_HEALTH===\n0, GPU-uuid, 0, 0x0000000000000004\n",
            "===DCGM===\n",
            "===IPMI===\nInstantaneous power reading: 300 Watts\n",
            "===XID===\nNVRM: Xid (PCI:0000:01:00): 79, pid=1\n",
            "===AVAILABILITY===\nsensors=1\nnvidia_smi=1\ndcgm=0\nipmitool=1\n",
            "===END===\n",
        );
        let telemetry = parse_telemetry(fixture).unwrap();
        assert_eq!(telemetry["summary"]["cpu_util_percent"], 50.0);
        assert_eq!(telemetry["summary"]["gpu_max_temperature_c"], 60.0);
        assert_eq!(telemetry["temperatures"].as_array().unwrap().len(), 1);
        assert_eq!(telemetry["temperatures"][0]["celsius"], 55.0);
        assert_eq!(telemetry["gpus"][0]["throttle_reasons"], 4);
        assert_eq!(
            telemetry["gpus"][0]["throttle_reason_label"],
            "SW power cap"
        );
        assert_eq!(telemetry["network"][0]["rx_bps"], 1000.0);
        let metrics = metric_samples(&telemetry);
        assert!(metrics
            .iter()
            .any(|sample| sample.metric == "gpu.0.power_w"));
        assert!(metrics
            .iter()
            .any(|sample| sample.metric == "nic.eth0.rx_bps"));
        assert!(metrics
            .iter()
            .any(|sample| sample.metric == "mem.used_percent" && sample.value == 60.0));
        assert!(metrics
            .iter()
            .any(|sample| { sample.metric == "gpu.0.mem_used_percent" && sample.value == 50.0 }));
        assert!(metrics.iter().any(|sample| sample.metric == "gpu.xid_last"));
        assert!(metrics.len() < 100);

        let degraded = parse_telemetry(&fixture.replace(
            "{\"coretemp\":{\"Package\":{\"temp1_input\":55.0},\"Voltage\":{\"in0_input\":0.656},\"Fan\":{\"fan1_input\":1200}}}",
            "{not-json}",
        ))
        .unwrap();
        assert!(degraded["temperatures"].as_array().unwrap().is_empty());
        assert!(degraded["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning == "telemetry sensor JSON is invalid"));
    }
}
