//! NIC / network-topology inventory collection (ISSUE 39).
//!
//! Design: `docs/design/phase2/20-cluster-network-topology.md` §3–§4.
//! Agentless: one SSH script reads `ip -j -d link` / `ip -j addr` /
//! `ip -j neigh` / per-iface `ethtool` speed / `lldpctl -f json0`.
//! Low-frequency by design — the background poller refreshes a host's
//! scan only when `network_scanned_at` is older than
//! `PollerConfig::topology_refresh` (~5 min), so this never scales with
//! viewer count and barely registers next to the 1 Hz stats loop.
//!
//! LLDP output is collected and stored verbatim but NOT rendered yet —
//! switch-node promotion is an explicit non-goal of this stage (§9).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::inventory::{HostRecord, NetworkInterface};
use crate::ssh::{exec, SshError, SshTarget};

/// Interfaces that are never part of the cluster fabric: loopback,
/// container veth pairs, container/VM NAT bridges, and CNI overlay
/// devices. A superset of the 1 Hz NetworkStats collector's filter —
/// a missed virtual iface here doesn't just add a noisy counter, it
/// fabricates a shared "network" node (every libvirt host carries the
/// same 192.168.122.0/24 on virbr0). Plain `br0`-style uplink bridges
/// are kept: VM hosts legitimately put their main IP on them.
fn is_ignored_iface(name: &str) -> bool {
    const IGNORED_PREFIXES: [&str; 8] = [
        "veth", "br-", "virbr", "lxdbr", "incusbr", "cni", "flannel", "cilium",
    ];
    name == "lo"
        || name == "docker0"
        || name == "docker_gwbridge"
        || name == "kube-bridge"
        || IGNORED_PREFIXES.iter().any(|p| name.starts_with(p))
}

pub(crate) const TOPOLOGY_SCRIPT: &str = r#"
echo '===LINK==='
ip -j -d link show 2>/dev/null || echo '[]'
echo '===ADDR==='
ip -j addr show 2>/dev/null || echo '[]'
echo '===NEIGH==='
ip -j neigh show 2>/dev/null || echo '[]'
echo '===ETHTOOL==='
for i in $(ls /sys/class/net 2>/dev/null); do
  case "$i" in lo|veth*|br-*|docker0|docker_gwbridge|kube-bridge|virbr*|lxdbr*|incusbr*|cni*|flannel*|cilium*) continue;; esac
  s=$(ethtool "$i" 2>/dev/null | awk -F': ' '/Speed:/{print $2}')
  echo "$i ${s:-unknown}"
done
echo '===LLDP==='
command -v lldpctl >/dev/null 2>&1 && lldpctl -f json0 2>/dev/null || echo '{}'
"#;

/// Result of one topology scan.
pub(crate) struct TopologyScan {
    pub interfaces: Vec<NetworkInterface>,
    /// Raw `lldpctl -f json0` output when lldpd is installed and
    /// returned anything beyond `{}`. Stored for the future
    /// switch-promotion stage; never interpreted here.
    pub lldp_raw: Option<serde_json::Value>,
}

/// Run the scan script over SSH and parse it.
pub(crate) async fn collect_topology(target: &SshTarget) -> Result<TopologyScan, SshError> {
    let out = exec(target, TOPOLOGY_SCRIPT).await?;
    Ok(parse_topology_output(&out.stdout))
}

/// Parse the marker-delimited script output. Pure function — fixture
/// tested below without SSH.
pub(crate) fn parse_topology_output(stdout: &str) -> TopologyScan {
    let sections = split_sections(stdout);
    let empty = String::new();

    let mut ifaces = parse_ip_link(sections.get("LINK").unwrap_or(&empty));
    let addrs = parse_ip_addr(sections.get("ADDR").unwrap_or(&empty));
    let speeds = parse_ethtool_speeds(sections.get("ETHTOOL").unwrap_or(&empty));
    let neigh = parse_ip_neigh(sections.get("NEIGH").unwrap_or(&empty));

    for iface in ifaces.iter_mut() {
        if let Some((v4, v6)) = addrs.get(&iface.name) {
            iface.ipv4 = v4.clone();
            iface.ipv6 = v6.clone();
        }
        if let Some(mbps) = speeds.get(&iface.name) {
            iface.speed_mbps = Some(*mbps);
        }
        if let Some(macs) = neigh.get(&iface.name) {
            iface.neigh_macs = macs.clone();
        }
    }

    // VLAN sub-interfaces often report no ethtool speed of their own
    // (driver-dependent); inherit the parent's so a 100G fabric doesn't
    // render as "unknown" in the graph.
    let parent_speeds: HashMap<String, u32> = ifaces
        .iter()
        .filter_map(|i| i.speed_mbps.map(|s| (i.name.clone(), s)))
        .collect();
    for iface in ifaces.iter_mut() {
        if iface.speed_mbps.is_some() {
            continue;
        }
        let Some(parent) = iface.parent.as_deref() else {
            continue;
        };
        iface.speed_mbps = parent_speeds.get(parent).copied();
    }

    let lldp_raw = sections
        .get("LLDP")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "{}")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    TopologyScan {
        interfaces: ifaces,
        lldp_raw,
    }
}

fn split_sections(stdout: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let mut current: Option<String> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("===")
            .and_then(|rest| rest.strip_suffix("==="))
        {
            current = Some(name.to_string());
            out.entry(name.to_string()).or_default();
            continue;
        }
        if let Some(name) = current.as_ref() {
            let buf = out.entry(name.clone()).or_default();
            buf.push_str(line);
            buf.push('\n');
        }
    }
    out
}

/// `ip -j -d link show` → one NetworkInterface per non-ignored iface.
fn parse_ip_link(json: &str) -> Vec<NetworkInterface> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json.trim()).unwrap_or_default();
    let mut out = Vec::new();
    for row in rows {
        let Some(name) = row.get("ifname").and_then(|v| v.as_str()) else {
            continue;
        };
        if is_ignored_iface(name) {
            continue;
        }
        let linkinfo = row.get("linkinfo");
        let info_kind = linkinfo
            .and_then(|l| l.get("info_kind"))
            .and_then(|v| v.as_str());
        // Pure-virtual kinds are never cluster fabric, whatever their
        // name: veth (containers), dummy (placeholders), ifb (shaping).
        if matches!(info_kind, Some("veth" | "dummy" | "ifb")) {
            continue;
        }
        let kind = match (info_kind, row.get("link_type").and_then(|v| v.as_str())) {
            (Some(k), _) => Some(k.to_string()), // vlan | bond | bridge | …
            (None, Some("infiniband")) => Some("infiniband".to_string()),
            (None, Some(_)) => Some("ethernet".to_string()),
            (None, None) => None,
        };
        let vlan_id = linkinfo
            .and_then(|l| l.get("info_data"))
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_u64())
            .and_then(|v| u16::try_from(v).ok());
        out.push(NetworkInterface {
            name: name.to_string(),
            mac: row
                .get("address")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            kind,
            state: row
                .get("operstate")
                .and_then(|v| v.as_str())
                .map(|s| s.to_ascii_lowercase()),
            mtu: row.get("mtu").and_then(|v| v.as_u64()).map(|v| v as u32),
            speed_mbps: None,
            vlan_id,
            parent: row.get("link").and_then(|v| v.as_str()).map(str::to_string),
            ipv4: Vec::new(),
            ipv6: Vec::new(),
            neigh_macs: Vec::new(),
        });
    }
    out
}

type AddrMap = HashMap<String, (Vec<String>, Vec<String>)>;

/// `ip -j addr show` → iface → (ipv4 "addr/prefix", global ipv6).
/// Link-local v6 (scope "link") is noise for network grouping and is
/// dropped.
fn parse_ip_addr(json: &str) -> AddrMap {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json.trim()).unwrap_or_default();
    let mut out: AddrMap = HashMap::new();
    for row in rows {
        let Some(name) = row.get("ifname").and_then(|v| v.as_str()) else {
            continue;
        };
        let entry = out.entry(name.to_string()).or_default();
        let Some(infos) = row.get("addr_info").and_then(|v| v.as_array()) else {
            continue;
        };
        for info in infos {
            let (Some(family), Some(local), Some(prefix)) = (
                info.get("family").and_then(|v| v.as_str()),
                info.get("local").and_then(|v| v.as_str()),
                info.get("prefixlen").and_then(|v| v.as_u64()),
            ) else {
                continue;
            };
            let cidr = format!("{local}/{prefix}");
            match family {
                "inet" => entry.0.push(cidr),
                "inet6" if info.get("scope").and_then(|v| v.as_str()) != Some("link") => {
                    entry.1.push(cidr);
                }
                _ => {}
            }
        }
    }
    out
}

/// `"<iface> <speed>"` lines from the ethtool loop → iface → Mb/s.
/// Lines like `eno1 unknown` or `eno1 Unknown!` carry no speed.
fn parse_ethtool_speeds(text: &str) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(speed)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(mbps) = speed
            .strip_suffix("Mb/s")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        out.insert(name.to_string(), mbps);
    }
    out
}

/// `ip -j neigh show` → iface → neighbor MACs (FAILED entries dropped).
/// Used later (ISSUE 40) to verify that hosts inferred to share a
/// subnet really see each other at L2.
fn parse_ip_neigh(json: &str) -> HashMap<String, Vec<String>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json.trim()).unwrap_or_default();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (Some(dev), Some(mac)) = (
            row.get("dev").and_then(|v| v.as_str()),
            row.get("lladdr").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let failed = row
            .get("state")
            .and_then(|v| v.as_array())
            .is_some_and(|st| st.iter().any(|s| s.as_str() == Some("FAILED")));
        if failed || is_ignored_iface(dev) {
            continue;
        }
        let entry = out.entry(dev.to_string()).or_default();
        let mac = mac.to_ascii_lowercase();
        if !entry.contains(&mac) {
            entry.push(mac);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Graph model (ISSUE 40) — design doc 20 §5.
//
// Pure inventory transform: no SSH, no DB. One call returns the whole
// graph, so cost is independent of how many viewers poll it.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TopologyGraph {
    pub generated_at: DateTime<Utc>,
    pub hosts: Vec<TopoHost>,
    pub networks: Vec<TopoNetwork>,
    pub links: Vec<TopoLink>,
}

#[derive(Debug, Serialize)]
pub struct TopoHost {
    pub id: Uuid,
    pub host: String,
    pub alias: Option<String>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub gpus: usize,
    pub ifaces: Vec<TopoHostIface>,
}

#[derive(Debug, Serialize)]
pub struct TopoHostIface {
    pub name: String,
    /// `None` for interfaces with no IPv4 (e.g. unconfigured IB ports,
    /// bond members) — shown as unattached in the UI.
    pub network_key: Option<String>,
    pub speed_mbps: Option<u32>,
    pub state: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TopoNetwork {
    /// `"vlan110/10.0.110.0/24"` or `"untagged/10.0.0.0/24"`.
    pub key: String,
    /// `VLAN110` for tagged networks, the subnet string otherwise.
    pub label: String,
    pub vlan_id: Option<u16>,
    pub subnet: String,
    /// Highest member link speed: `"100G"`, `"1G"`, `"unknown"`.
    pub speed_class: String,
    pub member_count: usize,
    /// True when at least one member saw another member's MAC in its
    /// neighbor table — subnet inference confirmed at L2 (§3.2).
    pub verified: bool,
}

#[derive(Debug, Serialize)]
pub struct TopoLink {
    pub host_id: Uuid,
    pub network_key: String,
    pub iface: String,
    pub speed_mbps: Option<u32>,
    pub state: Option<String>,
}

/// `"10.0.110.5/24"` → `"10.0.110.0/24"` (the network address).
fn ipv4_network(cidr: &str) -> Option<String> {
    let (ip, prefix) = cidr.split_once('/')?;
    let ip: std::net::Ipv4Addr = ip.parse().ok()?;
    let prefix: u32 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let net = u32::from(ip) & mask;
    Some(format!("{}/{}", std::net::Ipv4Addr::from(net), prefix))
}

fn speed_class(max_mbps: Option<u32>) -> String {
    match max_mbps {
        None => "unknown".to_string(),
        Some(m) if m >= 1000 => format!("{}G", m / 1000),
        Some(m) => format!("{m}M"),
    }
}

#[derive(Default)]
struct NetworkAccum {
    vlan_id: Option<u16>,
    subnet: String,
    members: HashSet<Uuid>,
    max_speed: Option<u32>,
    member_macs: HashSet<String>,
    seen_neigh_macs: HashSet<String>,
}

/// Build the cluster graph from inventory records.
pub fn build_topology_graph(records: &[HostRecord], generated_at: DateTime<Utc>) -> TopologyGraph {
    let mut hosts = Vec::with_capacity(records.len());
    let mut links: Vec<TopoLink> = Vec::new();
    let mut nets: HashMap<String, NetworkAccum> = HashMap::new();
    // (host, iface, network) triples already linked — an iface holding
    // a secondary address in the same subnet must not emit a second
    // edge (duplicate element ids abort the whole cytoscape render).
    let mut seen_links: HashSet<(Uuid, String, String)> = HashSet::new();

    for rec in records {
        let mut ifaces = Vec::new();
        for nic in &rec.network_interfaces {
            let mut keys: Vec<String> = Vec::new();
            for cidr in &nic.ipv4 {
                let Some(subnet) = ipv4_network(cidr) else {
                    continue;
                };
                // A /32 host route (keepalived VIP, tailscale CGNAT
                // address) can't have other members by definition — it
                // would render as a one-host "network" bubble per
                // address. Treat the iface as unattached instead.
                if subnet.ends_with("/32") {
                    continue;
                }
                let vlan_part = match nic.vlan_id {
                    Some(id) => format!("vlan{id}"),
                    None => "untagged".to_string(),
                };
                let key = format!("{vlan_part}/{subnet}");
                if !seen_links.insert((rec.id, nic.name.clone(), key.clone())) {
                    continue;
                }
                let acc = nets.entry(key.clone()).or_default();
                acc.vlan_id = nic.vlan_id.or(acc.vlan_id);
                acc.subnet = subnet;
                acc.members.insert(rec.id);
                acc.max_speed = acc.max_speed.max(nic.speed_mbps);
                if let Some(mac) = nic.mac.as_ref() {
                    acc.member_macs.insert(mac.to_ascii_lowercase());
                }
                acc.seen_neigh_macs
                    .extend(nic.neigh_macs.iter().map(|m| m.to_ascii_lowercase()));
                links.push(TopoLink {
                    host_id: rec.id,
                    network_key: key.clone(),
                    iface: nic.name.clone(),
                    speed_mbps: nic.speed_mbps,
                    state: nic.state.clone(),
                });
                keys.push(key);
            }
            if keys.is_empty() {
                ifaces.push(TopoHostIface {
                    name: nic.name.clone(),
                    network_key: None,
                    speed_mbps: nic.speed_mbps,
                    state: nic.state.clone(),
                });
            } else {
                for key in keys {
                    ifaces.push(TopoHostIface {
                        name: nic.name.clone(),
                        network_key: Some(key),
                        speed_mbps: nic.speed_mbps,
                        state: nic.state.clone(),
                    });
                }
            }
        }
        hosts.push(TopoHost {
            id: rec.id,
            host: rec.host.clone(),
            alias: rec.alias.clone(),
            last_ok_at: rec.last_ok_at,
            gpus: rec.gpus.len(),
            ifaces,
        });
    }

    let mut networks: Vec<TopoNetwork> = nets
        .into_iter()
        .map(|(key, acc)| {
            let label = match acc.vlan_id {
                Some(id) => format!("VLAN{id}"),
                None => acc.subnet.clone(),
            };
            // Verified when some member's neighbor table contains
            // another member's MAC. A host always sees its own MACs
            // locally, so self-only intersections don't count — the
            // neigh table never lists the host's own interface.
            let verified = acc
                .seen_neigh_macs
                .intersection(&acc.member_macs)
                .next()
                .is_some();
            TopoNetwork {
                key,
                label,
                vlan_id: acc.vlan_id,
                subnet: acc.subnet,
                speed_class: speed_class(acc.max_speed),
                member_count: acc.members.len(),
                verified,
            }
        })
        .collect();
    networks.sort_by(|a, b| a.key.cmp(&b.key));

    TopologyGraph {
        generated_at,
        hosts,
        networks,
        links,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
===LINK===
[
  {"ifname":"lo","link_type":"loopback","operstate":"UNKNOWN","mtu":65536},
  {"ifname":"eno1","address":"aa:bb:cc:00:00:01","link_type":"ether","operstate":"UP","mtu":1500},
  {"ifname":"enp1s0f0","address":"aa:bb:cc:00:00:02","link_type":"ether","operstate":"UP","mtu":9000},
  {"ifname":"enp1s0f0.110","address":"aa:bb:cc:00:00:02","link_type":"ether","operstate":"UP","mtu":9000,
   "link":"enp1s0f0","linkinfo":{"info_kind":"vlan","info_data":{"protocol":"802.1Q","id":110}}},
  {"ifname":"bond0","address":"aa:bb:cc:00:00:03","link_type":"ether","operstate":"UP","mtu":1500,
   "linkinfo":{"info_kind":"bond","info_data":{"mode":"802.3ad"}}},
  {"ifname":"ib0","address":"00:00:10:86:ff:fe:00:00:01","link_type":"infiniband","operstate":"UP","mtu":2044},
  {"ifname":"veth12ab","address":"de:ad:be:ef:00:01","link_type":"ether","operstate":"UP","mtu":1500},
  {"ifname":"docker0","address":"de:ad:be:ef:00:02","link_type":"ether","operstate":"DOWN","mtu":1500},
  {"ifname":"virbr0","address":"52:54:00:aa:bb:cc","link_type":"ether","operstate":"DOWN","mtu":1500,
   "linkinfo":{"info_kind":"bridge"}},
  {"ifname":"dummy0","address":"de:ad:be:ef:00:03","link_type":"ether","operstate":"UNKNOWN","mtu":1500,
   "linkinfo":{"info_kind":"dummy"}}
]
===ADDR===
[
  {"ifname":"eno1","addr_info":[
    {"family":"inet","local":"10.0.0.5","prefixlen":24,"scope":"global"},
    {"family":"inet6","local":"fe80::1","prefixlen":64,"scope":"link"}]},
  {"ifname":"enp1s0f0.110","addr_info":[
    {"family":"inet","local":"10.0.110.5","prefixlen":24,"scope":"global"},
    {"family":"inet6","local":"fd00:110::5","prefixlen":64,"scope":"global"}]},
  {"ifname":"ib0","addr_info":[]}
]
===NEIGH===
[
  {"dst":"10.0.0.1","dev":"eno1","lladdr":"AA:BB:CC:00:00:99","state":["REACHABLE"]},
  {"dst":"10.0.0.7","dev":"eno1","lladdr":"aa:bb:cc:00:00:07","state":["STALE"]},
  {"dst":"10.0.0.8","dev":"eno1","state":["FAILED"]},
  {"dst":"10.0.110.9","dev":"enp1s0f0.110","lladdr":"aa:bb:cc:00:00:09","state":["REACHABLE"]},
  {"dst":"172.17.0.2","dev":"docker0","lladdr":"02:42:ac:11:00:02","state":["REACHABLE"]}
]
===ETHTOOL===
eno1 1000Mb/s
enp1s0f0 100000Mb/s
enp1s0f0.110 unknown
bond0 2000Mb/s
ib0 unknown
===LLDP===
{"lldp":{"interface":[{"eno1":{"chassis":{"sw-mgmt":{}}}}]}}
"#;

    fn scan() -> TopologyScan {
        parse_topology_output(FIXTURE)
    }

    #[test]
    fn ignored_ifaces_are_dropped() {
        let names: Vec<_> = scan().interfaces.iter().map(|i| i.name.clone()).collect();
        assert!(!names.contains(&"lo".to_string()));
        assert!(!names.contains(&"veth12ab".to_string()));
        assert!(!names.contains(&"docker0".to_string()));
        // NAT bridge by name, placeholder by linkinfo kind (ISSUE 43).
        assert!(!names.contains(&"virbr0".to_string()));
        assert!(!names.contains(&"dummy0".to_string()));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn vlan_iface_carries_id_and_parent() {
        let s = scan();
        let vlan = s
            .interfaces
            .iter()
            .find(|i| i.name == "enp1s0f0.110")
            .unwrap();
        assert_eq!(vlan.vlan_id, Some(110));
        assert_eq!(vlan.parent.as_deref(), Some("enp1s0f0"));
        assert_eq!(vlan.kind.as_deref(), Some("vlan"));
        assert_eq!(vlan.ipv4, vec!["10.0.110.5/24".to_string()]);
        assert_eq!(vlan.ipv6, vec!["fd00:110::5/64".to_string()]);
    }

    #[test]
    fn bond_and_infiniband_kinds_detected() {
        let s = scan();
        let bond = s.interfaces.iter().find(|i| i.name == "bond0").unwrap();
        assert_eq!(bond.kind.as_deref(), Some("bond"));
        assert_eq!(bond.speed_mbps, Some(2000));
        let ib = s.interfaces.iter().find(|i| i.name == "ib0").unwrap();
        assert_eq!(ib.kind.as_deref(), Some("infiniband"));
        assert_eq!(ib.speed_mbps, None); // ethtool said unknown
        assert!(ib.ipv4.is_empty());
    }

    #[test]
    fn link_local_v6_dropped_and_speed_parsed() {
        let s = scan();
        let mgmt = s.interfaces.iter().find(|i| i.name == "eno1").unwrap();
        assert_eq!(mgmt.ipv4, vec!["10.0.0.5/24".to_string()]);
        assert!(mgmt.ipv6.is_empty()); // fe80:: was scope=link
        assert_eq!(mgmt.speed_mbps, Some(1000));
        assert_eq!(mgmt.state.as_deref(), Some("up"));
        assert_eq!(mgmt.mtu, Some(1500));
    }

    #[test]
    fn neigh_macs_lowercased_deduped_failed_dropped() {
        let s = scan();
        let mgmt = s.interfaces.iter().find(|i| i.name == "eno1").unwrap();
        assert_eq!(
            mgmt.neigh_macs,
            vec![
                "aa:bb:cc:00:00:99".to_string(),
                "aa:bb:cc:00:00:07".to_string()
            ]
        );
    }

    #[test]
    fn vlan_iface_inherits_parent_speed() {
        // ethtool said "unknown" for enp1s0f0.110; the parent reported
        // 100000 — the sub-interface inherits it.
        let s = scan();
        let vlan = s
            .interfaces
            .iter()
            .find(|i| i.name == "enp1s0f0.110")
            .unwrap();
        assert_eq!(vlan.speed_mbps, Some(100000));
    }

    #[test]
    fn lldp_raw_kept_when_present() {
        assert!(scan().lldp_raw.is_some());
        let empty = parse_topology_output("===LLDP===\n{}\n");
        assert!(empty.lldp_raw.is_none());
    }

    fn nic(
        name: &str,
        mac: &str,
        vlan: Option<u16>,
        speed: Option<u32>,
        ipv4: &[&str],
        neigh: &[&str],
    ) -> NetworkInterface {
        NetworkInterface {
            name: name.into(),
            mac: Some(mac.into()),
            kind: Some("ethernet".into()),
            state: Some("up".into()),
            mtu: Some(1500),
            speed_mbps: speed,
            vlan_id: vlan,
            parent: None,
            ipv4: ipv4.iter().map(|s| s.to_string()).collect(),
            ipv6: vec![],
            neigh_macs: neigh.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn host(alias: &str, nics: Vec<NetworkInterface>) -> HostRecord {
        HostRecord {
            id: Uuid::new_v4(),
            host: format!("{alias}.local"),
            ssh_user: "ops".into(),
            ssh_port: 22,
            key_path: "/tmp/k".into(),
            created_at: Utc::now(),
            last_ok_at: None,
            tenant_id: Uuid::nil(),
            machine_id: None,
            dmi_uuid: None,
            dmi_serial: None,
            alias: Some(alias.into()),
            cpu_model: None,
            cpu_cores: None,
            gpus: vec![],
            gadgetini: None,
            network_interfaces: nics,
            network_scanned_at: None,
            lldp_raw: None,
        }
    }

    #[test]
    fn graph_groups_hosts_by_vlan_and_subnet() {
        let a = host(
            "a",
            vec![
                nic(
                    "eno1",
                    "aa:00:00:00:00:01",
                    None,
                    Some(1000),
                    &["10.0.0.5/24"],
                    &["aa:00:00:00:00:02"],
                ),
                nic(
                    "enp1.110",
                    "aa:00:00:00:00:11",
                    Some(110),
                    Some(100000),
                    &["10.0.110.5/24"],
                    &[],
                ),
            ],
        );
        let b = host(
            "b",
            vec![
                nic(
                    "eno1",
                    "aa:00:00:00:00:02",
                    None,
                    Some(1000),
                    &["10.0.0.6/24"],
                    &[],
                ),
                nic(
                    "enp1.110",
                    "aa:00:00:00:00:12",
                    Some(110),
                    Some(100000),
                    &["10.0.110.6/24"],
                    &[],
                ),
                nic(
                    "enp1.120",
                    "aa:00:00:00:00:22",
                    Some(120),
                    Some(100000),
                    &["10.0.120.6/24"],
                    &[],
                ),
            ],
        );
        let g = build_topology_graph(&[a, b], Utc::now());

        assert_eq!(g.hosts.len(), 2);
        assert_eq!(g.networks.len(), 3);
        let mgmt = g.networks.iter().find(|n| n.vlan_id.is_none()).unwrap();
        assert_eq!(mgmt.key, "untagged/10.0.0.0/24");
        assert_eq!(mgmt.label, "10.0.0.0/24");
        assert_eq!(mgmt.member_count, 2);
        assert_eq!(mgmt.speed_class, "1G");
        // host a saw host b's MAC on the mgmt net → verified.
        assert!(mgmt.verified);

        let v110 = g.networks.iter().find(|n| n.vlan_id == Some(110)).unwrap();
        assert_eq!(v110.label, "VLAN110");
        assert_eq!(v110.member_count, 2);
        assert_eq!(v110.speed_class, "100G");
        assert!(!v110.verified); // no cross-member neigh evidence

        let v120 = g.networks.iter().find(|n| n.vlan_id == Some(120)).unwrap();
        assert_eq!(v120.member_count, 1);

        assert_eq!(g.links.len(), 5); // 2 + 3 (one per iface-with-ipv4)
    }

    #[test]
    fn secondary_addr_in_same_subnet_emits_single_link() {
        let a = host(
            "a",
            vec![nic(
                "eno1",
                "aa:00:00:00:00:01",
                None,
                Some(1000),
                &["10.0.0.5/24", "10.0.0.6/24"],
                &[],
            )],
        );
        let g = build_topology_graph(&[a], Utc::now());
        assert_eq!(g.links.len(), 1);
        assert_eq!(g.hosts[0].ifaces.len(), 1);
        assert_eq!(g.networks.len(), 1);
        assert_eq!(g.networks[0].member_count, 1);
    }

    #[test]
    fn host_route_slash32_is_unattached() {
        // keepalived VIPs / tailscale 100.x addresses are /32 — no
        // shared network can exist behind them.
        let a = host(
            "a",
            vec![nic(
                "tailscale0",
                "aa:00:00:00:00:44",
                None,
                None,
                &["100.64.0.5/32"],
                &[],
            )],
        );
        let g = build_topology_graph(&[a], Utc::now());
        assert!(g.networks.is_empty());
        assert!(g.links.is_empty());
        assert_eq!(g.hosts[0].ifaces.len(), 1);
        assert!(g.hosts[0].ifaces[0].network_key.is_none());
    }

    #[test]
    fn ipv4_less_iface_is_unattached_not_linked() {
        let a = host(
            "a",
            vec![nic("ib0", "aa:00:00:00:00:33", None, None, &[], &[])],
        );
        let g = build_topology_graph(&[a], Utc::now());
        assert!(g.networks.is_empty());
        assert!(g.links.is_empty());
        assert_eq!(g.hosts[0].ifaces.len(), 1);
        assert!(g.hosts[0].ifaces[0].network_key.is_none());
    }

    #[test]
    fn ipv4_network_math() {
        assert_eq!(
            ipv4_network("10.0.110.5/24").as_deref(),
            Some("10.0.110.0/24")
        );
        assert_eq!(
            ipv4_network("192.168.1.130/25").as_deref(),
            Some("192.168.1.128/25")
        );
        assert_eq!(ipv4_network("10.1.2.3/8").as_deref(), Some("10.0.0.0/8"));
        assert_eq!(ipv4_network("bogus/24"), None);
        assert_eq!(ipv4_network("10.0.0.1/33"), None);
    }

    #[test]
    fn malformed_sections_yield_empty_scan() {
        let s = parse_topology_output("===LINK===\nnot json\n===ADDR===\n[broken\n");
        assert!(s.interfaces.is_empty());
        assert!(s.lldp_raw.is_none());
    }
}
