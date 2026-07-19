use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_INTERFACES: usize = 128;
const MAX_ROUTES: usize = 256;
const MAX_NEIGHBORS: usize = 256;
const MAX_LLDP_NEIGHBORS: usize = 128;

pub(crate) fn parse_topology(stdout: &str) -> Result<Value, &'static str> {
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err("topology output exceeded the Bundle parser ceiling");
    }
    let sections = split_sections(stdout)?;
    let links = parse_links(required_section(&sections, "LINK")?)?;
    let addresses = parse_addresses(required_section(&sections, "ADDR")?)?;
    let neighbors = parse_neighbors(required_section(&sections, "NEIGH")?)?;
    let speeds = parse_speeds(required_section(&sections, "ETHTOOL")?)?;
    let routes = parse_routes(required_section(&sections, "ROUTE")?)?;
    let mut interfaces = Vec::new();
    for mut interface in links {
        let Some(name) = interface
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let address = addresses.get(&name);
        let speed = speeds.get(&name).copied().or_else(|| {
            interface
                .get("parent")
                .and_then(Value::as_str)
                .and_then(|parent| speeds.get(parent).copied())
        });
        let object = interface
            .as_object_mut()
            .expect("parsed interface is always an object");
        object.insert(
            "ipv4".into(),
            json!(address.map(|value| &value.0).cloned().unwrap_or_default()),
        );
        object.insert(
            "ipv6".into(),
            json!(address.map(|value| &value.1).cloned().unwrap_or_default()),
        );
        object.insert("speed_mbps".into(), speed.map_or(Value::Null, Value::from));
        object.insert(
            "neighbors".into(),
            json!(neighbors.get(&name).cloned().unwrap_or_default()),
        );
        interfaces.push(interface);
    }
    let (lldp_neighbors, lldp_warning) =
        parse_lldp(sections.get("LLDP").map_or("{}", String::as_str));
    let mut warnings = Vec::new();
    if let Some(warning) = lldp_warning {
        warnings.push(warning);
    }
    Ok(json!({
        "interfaces": interfaces,
        "routes": routes,
        "lldp_neighbors": lldp_neighbors,
        "availability": {
            "ip": true,
            "ethtool": sections.get("AVAILABILITY").is_some_and(|value| value.contains("ethtool=1")),
            "lldp": sections.get("AVAILABILITY").is_some_and(|value| value.contains("lldp=1")),
        },
        "warnings": warnings,
    }))
}

fn split_sections(stdout: &str) -> Result<BTreeMap<String, String>, &'static str> {
    const ALLOWED: [&str; 7] = [
        "LINK",
        "ADDR",
        "NEIGH",
        "ROUTE",
        "ETHTOOL",
        "LLDP",
        "AVAILABILITY",
    ];
    let mut sections = BTreeMap::new();
    let mut current: Option<&str> = None;
    let mut body = String::new();
    for line in stdout.lines() {
        if let Some(tag) = line
            .strip_prefix("===")
            .and_then(|value| value.strip_suffix("==="))
        {
            if let Some(name) = current.take() {
                if sections
                    .insert(name.to_string(), std::mem::take(&mut body))
                    .is_some()
                {
                    return Err("topology output repeated a signed section");
                }
            }
            if tag == "END" {
                current = None;
            } else if ALLOWED.contains(&tag) {
                current = Some(tag);
            } else {
                return Err("topology output contained an unknown section");
            }
        } else if current.is_some() {
            body.push_str(line);
            body.push('\n');
        } else if !line.trim().is_empty() {
            return Err("topology output escaped its signed sections");
        }
    }
    if let Some(name) = current {
        sections.insert(name.to_string(), body);
    }
    Ok(sections)
}

fn required_section<'a>(
    sections: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, &'static str> {
    sections
        .get(name)
        .map(String::as_str)
        .ok_or("topology output omitted a required signed section")
}

fn json_rows(section: &str, kind: &'static str) -> Result<Vec<Value>, &'static str> {
    let value: Value = serde_json::from_str(section.trim()).map_err(|_| kind)?;
    value.as_array().cloned().ok_or(kind)
}

fn parse_links(section: &str) -> Result<Vec<Value>, &'static str> {
    let rows = json_rows(section, "topology link JSON is invalid")?;
    if rows.len() > MAX_INTERFACES {
        return Err("topology interface list exceeded its ceiling");
    }
    let mut interfaces = Vec::new();
    for row in rows {
        let Some(name) = row.get("ifname").and_then(Value::as_str) else {
            continue;
        };
        if ignored_interface(name) || name.len() > 64 {
            continue;
        }
        let link_info = row.get("linkinfo");
        let info_kind = link_info
            .and_then(|value| value.get("info_kind"))
            .and_then(Value::as_str);
        if matches!(info_kind, Some("veth" | "dummy" | "ifb")) {
            continue;
        }
        let kind = info_kind
            .or_else(|| {
                (row.get("link_type").and_then(Value::as_str) == Some("infiniband"))
                    .then_some("infiniband")
            })
            .unwrap_or("ethernet");
        let vlan_id = link_info
            .and_then(|value| value.get("info_data"))
            .and_then(|value| value.get("id"))
            .and_then(Value::as_u64);
        interfaces.push(json!({
            "name": name,
            "mac": bounded_string(row.get("address").and_then(Value::as_str), 128),
            "kind": kind,
            "state": row.get("operstate").and_then(Value::as_str).map(str::to_ascii_lowercase),
            "mtu": row.get("mtu").and_then(Value::as_u64),
            "vlan_id": vlan_id,
            "parent": bounded_string(row.get("link").and_then(Value::as_str), 64),
            "master": bounded_string(row.get("master").and_then(Value::as_str), 64),
        }));
    }
    Ok(interfaces)
}

type AddressMap = HashMap<String, (Vec<String>, Vec<String>)>;

fn parse_addresses(section: &str) -> Result<AddressMap, &'static str> {
    let rows = json_rows(section, "topology address JSON is invalid")?;
    let mut addresses = HashMap::new();
    for row in rows.into_iter().take(MAX_INTERFACES) {
        let Some(name) = row.get("ifname").and_then(Value::as_str) else {
            continue;
        };
        if ignored_interface(name) {
            continue;
        }
        let entry: &mut (Vec<String>, Vec<String>) = addresses.entry(name.to_string()).or_default();
        for address in row
            .get("addr_info")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(64)
        {
            let (Some(family), Some(local), Some(prefix)) = (
                address.get("family").and_then(Value::as_str),
                address.get("local").and_then(Value::as_str),
                address.get("prefixlen").and_then(Value::as_u64),
            ) else {
                continue;
            };
            if local.len() > 64 || prefix > 128 {
                continue;
            }
            let cidr = format!("{local}/{prefix}");
            match family {
                "inet" => entry.0.push(cidr),
                "inet6" if address.get("scope").and_then(Value::as_str) != Some("link") => {
                    entry.1.push(cidr);
                }
                _ => {}
            }
        }
    }
    Ok(addresses)
}

fn parse_neighbors(section: &str) -> Result<HashMap<String, Vec<Value>>, &'static str> {
    let rows = json_rows(section, "topology neighbor JSON is invalid")?;
    if rows.len() > MAX_NEIGHBORS {
        return Err("topology neighbor list exceeded its ceiling");
    }
    let mut neighbors: HashMap<String, Vec<Value>> = HashMap::new();
    for row in rows {
        let Some(interface) = row.get("dev").and_then(Value::as_str) else {
            continue;
        };
        if ignored_interface(interface) {
            continue;
        }
        let failed = row
            .get("state")
            .and_then(Value::as_array)
            .is_some_and(|states| states.iter().any(|state| state == "FAILED"));
        if failed {
            continue;
        }
        neighbors
            .entry(interface.to_string())
            .or_default()
            .push(json!({
                "address": bounded_string(row.get("dst").and_then(Value::as_str), 64),
                "mac": bounded_string(row.get("lladdr").and_then(Value::as_str), 128)
                    .map(|value| value.to_ascii_lowercase()),
                "state": row.get("state").cloned().unwrap_or(Value::Null),
            }));
    }
    Ok(neighbors)
}

fn parse_speeds(section: &str) -> Result<HashMap<String, u64>, &'static str> {
    let mut speeds = HashMap::new();
    for line in section.lines().take(MAX_INTERFACES) {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(raw)) = (fields.next(), fields.next()) else {
            continue;
        };
        if name.len() > 64 {
            return Err("topology ethtool interface is invalid");
        }
        if let Some(speed) = raw
            .strip_suffix("Mb/s")
            .and_then(|value| value.parse::<u64>().ok())
        {
            speeds.insert(name.to_string(), speed);
        }
    }
    Ok(speeds)
}

fn parse_routes(section: &str) -> Result<Vec<Value>, &'static str> {
    let rows = json_rows(section, "topology route JSON is invalid")?;
    if rows.len() > MAX_ROUTES {
        return Err("topology route list exceeded its ceiling");
    }
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let destination = row.get("dst").and_then(Value::as_str).unwrap_or("default");
            if destination.len() > 128 {
                return None;
            }
            Some(json!({
                "destination": destination,
                "gateway": bounded_string(row.get("gateway").and_then(Value::as_str), 64),
                "interface": bounded_string(row.get("dev").and_then(Value::as_str), 64),
                "protocol": bounded_string(row.get("protocol").and_then(Value::as_str), 64),
                "scope": bounded_string(row.get("scope").and_then(Value::as_str), 64),
                "metric": row.get("metric").and_then(Value::as_u64),
            }))
        })
        .collect())
}

fn parse_lldp(section: &str) -> (Vec<Value>, Option<String>) {
    if section.trim().is_empty() || section.trim() == "{}" {
        return (Vec::new(), None);
    }
    let Ok(root) = serde_json::from_str::<Value>(section) else {
        return (
            Vec::new(),
            Some("LLDP returned invalid JSON; typed peers were not updated".into()),
        );
    };
    let Some(interfaces) = root
        .get("lldp")
        .and_then(|value| value.get("interface"))
        .and_then(Value::as_object)
    else {
        return (Vec::new(), None);
    };
    let mut neighbors = Vec::new();
    for (local_interface, peer) in interfaces.iter().take(MAX_LLDP_NEIGHBORS) {
        let peer = peer.as_object().and_then(|object| {
            if object.len() == 1 {
                object.values().next().and_then(Value::as_object)
            } else {
                Some(object)
            }
        });
        let Some(peer) = peer else { continue };
        let chassis = peer
            .get("chassis")
            .and_then(first_named_object)
            .cloned()
            .unwrap_or_default();
        let port = peer
            .get("port")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let chassis_id = nested_value(&chassis, "id", "value");
        let port_id = nested_value(&port, "id", "value");
        if chassis_id.is_none() && port_id.is_none() {
            continue;
        }
        neighbors.push(json!({
            "local_interface": local_interface,
            "chassis_id": bounded_string(chassis_id, 256),
            "system_name": bounded_string(chassis.get("name").and_then(Value::as_str), 256),
            "system_description": bounded_string(chassis.get("descr").and_then(Value::as_str), 512),
            "port_id": bounded_string(port_id, 256),
            "port_description": bounded_string(port.get("descr").and_then(Value::as_str), 256),
        }));
    }
    (neighbors, None)
}

fn first_named_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    let object = value.as_object()?;
    if object.contains_key("id") || object.contains_key("name") {
        Some(object)
    } else {
        object.values().find_map(Value::as_object)
    }
}

fn nested_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    parent: &str,
    child: &str,
) -> Option<&'a str> {
    object
        .get(parent)
        .and_then(|value| value.get(child))
        .and_then(Value::as_str)
}

fn bounded_string(value: Option<&str>, maximum: usize) -> Option<String> {
    value
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(|value| value.chars().take(maximum).collect())
}

fn ignored_interface(name: &str) -> bool {
    name == "lo"
        || name == "docker0"
        || name == "docker_gwbridge"
        || [
            "veth", "br-", "virbr", "lxdbr", "incusbr", "cni", "flannel", "cilium",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[derive(Default)]
struct NetworkAccumulator {
    vlan_id: Option<u64>,
    subnet: String,
    members: BTreeSet<String>,
    member_macs: BTreeSet<String>,
    neighbor_macs: BTreeSet<String>,
    max_speed_mbps: Option<u64>,
}

pub(crate) fn build_topology_graph(rows: &[BTreeMap<String, Value>]) -> Value {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut networks: BTreeMap<String, NetworkAccumulator> = BTreeMap::new();
    let mut switches = BTreeSet::new();
    for row in rows {
        let Some(target_id) = row.get("target_id").and_then(Value::as_str) else {
            continue;
        };
        let inventory = row.get("inventory").unwrap_or(&Value::Null);
        let topology = row.get("topology").unwrap_or(&Value::Null);
        let host_node = format!("host:{target_id}");
        nodes.push(json!({
            "id": host_node,
            "label": inventory.get("hostname").and_then(Value::as_str).unwrap_or(target_id),
            "kind": "host",
            "target_id": target_id,
            "status": row.get("health_status").cloned().unwrap_or(Value::Null),
            "health_status": row.get("health_status").cloned().unwrap_or(Value::Null),
            "gpu_count": inventory.get("gpu_count").and_then(Value::as_u64).unwrap_or(0),
            "observed_at": row.get("observed_at").cloned().unwrap_or(Value::Null),
        }));
        for interface in topology
            .get("interfaces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = interface
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            for cidr in interface
                .get("ipv4")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let Some(subnet) = ipv4_network(cidr) else {
                    continue;
                };
                if subnet.ends_with("/32") {
                    continue;
                }
                let vlan_id = interface.get("vlan_id").and_then(Value::as_u64);
                let key = format!(
                    "{}/{}",
                    vlan_id.map_or_else(|| "untagged".into(), |id| format!("vlan{id}")),
                    subnet
                );
                let network = networks.entry(key.clone()).or_default();
                network.vlan_id = vlan_id.or(network.vlan_id);
                network.subnet = subnet;
                network.members.insert(target_id.to_string());
                network.max_speed_mbps = network
                    .max_speed_mbps
                    .max(interface.get("speed_mbps").and_then(Value::as_u64));
                if let Some(mac) = interface.get("mac").and_then(Value::as_str) {
                    network.member_macs.insert(mac.to_ascii_lowercase());
                }
                for mac in interface
                    .get("neighbors")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|neighbor| neighbor.get("mac").and_then(Value::as_str))
                {
                    network.neighbor_macs.insert(mac.to_ascii_lowercase());
                }
                edges.push(json!({
                    "id": format!("{host_node}/network:{key}/{name}"),
                    "source": host_node,
                    "target": format!("network:{key}"),
                    "kind": "membership",
                    "interface": name,
                    "state": interface.get("state").cloned().unwrap_or(Value::Null),
                    "speed_mbps": interface.get("speed_mbps").cloned().unwrap_or(Value::Null),
                }));
            }
        }
        for neighbor in topology
            .get("lldp_neighbors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let identity = neighbor
                .get("chassis_id")
                .or_else(|| neighbor.get("system_name"))
                .and_then(Value::as_str);
            let Some(identity) = identity else { continue };
            let switch_id = format!("switch:{}", short_digest(identity));
            if switches.insert(switch_id.clone()) {
                nodes.push(json!({
                    "id": switch_id,
                    "label": neighbor.get("system_name").and_then(Value::as_str).unwrap_or(identity),
                    "kind": "switch",
                    "chassis_id": neighbor.get("chassis_id").cloned().unwrap_or(Value::Null),
                }));
            }
            edges.push(json!({
                "id": format!("{host_node}/{switch_id}/{}", neighbor.get("local_interface").and_then(Value::as_str).unwrap_or("unknown")),
                "source": host_node,
                "target": switch_id,
                "kind": "lldp",
                "local_interface": neighbor.get("local_interface").cloned().unwrap_or(Value::Null),
                "remote_port": neighbor.get("port_id").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    let mut network_records = Vec::new();
    for (key, network) in networks {
        let verified = network
            .neighbor_macs
            .intersection(&network.member_macs)
            .next()
            .is_some();
        let label = network
            .vlan_id
            .map_or_else(|| network.subnet.clone(), |id| format!("VLAN{id}"));
        nodes.push(json!({
            "id": format!("network:{key}"),
            "label": label,
            "kind": "network",
            "subnet": network.subnet,
            "vlan_id": network.vlan_id,
            "member_count": network.members.len(),
            "speed_class": speed_class(network.max_speed_mbps),
            "verified": verified,
        }));
        network_records.push(json!({
            "key": key,
            "label": label,
            "subnet": network.subnet,
            "vlan_id": network.vlan_id,
            "member_count": network.members.len(),
            "speed_class": speed_class(network.max_speed_mbps),
            "verified": verified,
        }));
    }
    json!({
        "nodes": nodes,
        "edges": edges,
        "networks": network_records,
        "generated_at": crate::operational::now(),
    })
}

fn ipv4_network(cidr: &str) -> Option<String> {
    let (address, prefix) = cidr.split_once('/')?;
    let address: std::net::Ipv4Addr = address.parse().ok()?;
    let prefix: u32 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some(format!(
        "{}/{}",
        std::net::Ipv4Addr::from(u32::from(address) & mask),
        prefix
    ))
}

fn speed_class(speed_mbps: Option<u64>) -> String {
    match speed_mbps {
        Some(speed) if speed >= 1_000 => format!("{}G", speed / 1_000),
        Some(speed) => format!("{speed}M"),
        None => "unknown".into(),
    }
}

fn short_digest(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(&digest[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"===LINK===
[
  {"ifname":"lo","link_type":"loopback","operstate":"UNKNOWN","mtu":65536},
  {"ifname":"eno1","address":"aa:bb:cc:00:00:01","link_type":"ether","operstate":"UP","mtu":1500},
  {"ifname":"eno1.110","address":"aa:bb:cc:00:00:01","link_type":"ether","operstate":"UP","mtu":9000,"link":"eno1","linkinfo":{"info_kind":"vlan","info_data":{"id":110}}},
  {"ifname":"bond0","address":"aa:bb:cc:00:00:02","link_type":"ether","operstate":"UP","mtu":9000,"linkinfo":{"info_kind":"bond"}}
]
===ADDR===
[
  {"ifname":"eno1.110","addr_info":[{"family":"inet","local":"10.0.110.5","prefixlen":24,"scope":"global"}]},
  {"ifname":"bond0","addr_info":[{"family":"inet6","local":"fd00::5","prefixlen":64,"scope":"global"}]}
]
===NEIGH===
[{"dst":"10.0.110.6","dev":"eno1.110","lladdr":"aa:bb:cc:00:00:09","state":["REACHABLE"]}]
===ROUTE===
[{"dst":"default","gateway":"10.0.110.1","dev":"eno1.110","protocol":"static","metric":100}]
===ETHTOOL===
eno1 100000Mb/s
===LLDP===
{"lldp":{"interface":{"eno1":{"via":"LLDP","chassis":{"sw1":{"id":{"type":"mac","value":"00:11:22:33:44:55"},"name":"leaf-1"}},"port":{"id":{"type":"ifname","value":"Ethernet1"}}}}}}
===AVAILABILITY===
ethtool=1
lldp=1
===END===
"#;

    #[test]
    fn parser_preserves_vlan_parent_speed_neighbor_route_and_lldp() {
        let topology = parse_topology(FIXTURE).unwrap();
        assert_eq!(topology["interfaces"].as_array().unwrap().len(), 3);
        let vlan = topology["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|interface| interface["name"] == "eno1.110")
            .unwrap();
        assert_eq!(vlan["vlan_id"], 110);
        assert_eq!(vlan["parent"], "eno1");
        assert_eq!(vlan["speed_mbps"], 100000);
        assert_eq!(vlan["neighbors"][0]["address"], "10.0.110.6");
        assert_eq!(topology["routes"][0]["gateway"], "10.0.110.1");
        assert_eq!(topology["lldp_neighbors"][0]["system_name"], "leaf-1");
    }

    #[test]
    fn fleet_graph_builds_stable_host_network_and_switch_relations() {
        let topology = parse_topology(FIXTURE).unwrap();
        let rows = vec![BTreeMap::from([
            ("target_id".into(), json!("edge-one")),
            (
                "inventory".into(),
                json!({"hostname":"edge-one","gpu_count":1}),
            ),
            ("topology".into(), topology),
            ("observed_at".into(), json!("2026-07-12T00:00:00Z")),
            ("health_status".into(), json!("unreachable")),
        ])];
        let graph = build_topology_graph(&rows);
        assert!(graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "network:vlan110/10.0.110.0/24"));
        assert!(graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"] == "switch"));
        assert!(graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["kind"] == "lldp"));
        let host = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["kind"] == "host")
            .unwrap();
        assert_eq!(host["status"], "unreachable");
    }
}
