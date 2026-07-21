// Per-interface network throughput/stability, backed by sysinfo::Networks.
// No AppKit dependency here — plain data collection, unit-testable in
// isolation like storage.rs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

#[derive(Serialize, Deserialize, Clone)]
pub struct NetworkInterfaceInfo {
    pub name: String,
    // Human-facing label, e.g. "Wi-Fi (en0)" or "VPN (utun0)" — see
    // `display_name`. Falls back to the bare device name when nothing
    // recognizes it.
    pub display_name: String,
    // Bytes seen since the previous refresh, not a lifetime total — see
    // sysinfo::NetworkData::received()/transmitted(). The caller controls
    // how far apart refreshes happen (this app ticks every 5s, matching
    // the Storage window's refresh cadence), so turning this into a
    // bytes/sec rate is the caller's job if it wants one; exposed here as
    // the raw per-tick delta to avoid baking an assumed interval into this
    // module.
    pub download_bytes_per_tick: u64,
    pub upload_bytes_per_tick: u64,
    pub packet_errors: u64,
    pub ip_addresses: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    Stable,
    Unstable,
}

impl Stability {
    pub fn label(&self) -> &'static str {
        match self {
            Stability::Stable => "Stable",
            Stability::Unstable => "Unstable",
        }
    }
}

// Any receive/transmit errors since the last refresh mark an interface as
// unstable. Simple on purpose — this is a hobby project's at-a-glance
// indicator, not a production network diagnostic.
pub fn classify_stability(packet_errors: u64) -> Stability {
    if packet_errors > 0 {
        Stability::Unstable
    } else {
        Stability::Stable
    }
}

// Interfaces with no IP address and no traffic ever are inactive/unused
// (e.g. spare en* ports, disconnected utun tunnels) and just clutter the
// list — see the throwaway probe run against `Networks::new_with_refreshed_list`
// on a real Mac, which showed a dozen such interfaces alongside the few
// actually in use. Loopback and any interface carrying real traffic or
// holding an address stay.
fn is_interesting(data: &sysinfo::NetworkData) -> bool {
    !data.ip_networks().is_empty() || data.total_received() > 0 || data.total_transmitted() > 0
}

// There's no single system-wide "the network connection" — a Mac can have
// several active interfaces (Wi-Fi, Ethernet, a VPN's utun, etc). For the
// Status/Speed tabs' graphs we pick one "primary" interface per tick rather
// than drawing a chart per interface: the one carrying the most traffic
// this tick, since that's almost always whichever link the user is actually
// using. If every interface is idle (e.g. right after opening the window,
// before any traffic has moved), fall back to the first non-loopback
// interface that has an IP address, so the graphs still show *something*
// meaningful instead of picking en0 vs en1 arbitrarily.
pub fn select_primary(interfaces: &[NetworkInterfaceInfo]) -> Option<&NetworkInterfaceInfo> {
    interfaces
        .iter()
        .max_by_key(|i| i.download_bytes_per_tick + i.upload_bytes_per_tick)
        .filter(|i| i.download_bytes_per_tick + i.upload_bytes_per_tick > 0)
        .or_else(|| {
            interfaces
                .iter()
                .find(|i| i.name != "lo0" && !i.ip_addresses.is_empty())
        })
}

// Device names like "en0"/"utun0" are meaningless to a person — macOS
// itself knows en0 is "Wi-Fi" etc, but only exposes that mapping via
// `networksetup`, not through sysinfo. Shells out once per tick (cheap:
// it's a fast, local, no-network command), same pattern as
// app_coalitions.rs's `lsappinfo` call.
fn hardware_port_names() -> HashMap<String, String> {
    let output = match Command::new("networksetup")
        .arg("-listallhardwareports")
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return HashMap::new(),
    };
    let text = String::from_utf8_lossy(&output);

    let mut names = HashMap::new();
    let mut pending_port: Option<String> = None;
    for line in text.lines() {
        if let Some(port) = line.strip_prefix("Hardware Port: ") {
            pending_port = Some(port.trim().to_string());
        } else if let Some(device) = line.strip_prefix("Device: ")
            && let Some(port) = pending_port.take()
        {
            names.insert(device.trim().to_string(), port);
        }
    }
    names
}

// Covers the common virtual/system interfaces `networksetup` doesn't know
// about (it only lists physical hardware ports). Matched by prefix since
// macOS numbers these per-instance (utun0, utun1, ...).
fn well_known_label(name: &str) -> Option<&'static str> {
    if name == "lo0" {
        Some("Loopback")
    } else if name.starts_with("utun") {
        Some("VPN/Tunnel")
    } else if name.starts_with("awdl") {
        Some("AirDrop/Handoff")
    } else if name.starts_with("bridge") {
        Some("Bridge")
    } else if name.starts_with("ap") {
        Some("Personal Hotspot")
    } else if name.starts_with("llw") {
        Some("Low-Latency WLAN")
    } else if name.starts_with("gif") || name.starts_with("stf") {
        Some("Tunnel")
    } else {
        None
    }
}

fn display_name(name: &str, hardware_ports: &HashMap<String, String>) -> String {
    if let Some(port) = hardware_ports.get(name) {
        format!("{port} ({name})")
    } else if let Some(label) = well_known_label(name) {
        format!("{label} ({name})")
    } else {
        name.to_string()
    }
}

// Call this once per refresh tick, reusing the same `Networks` instance
// across calls — `received()`/`transmitted()`/`errors_on_*()` are deltas
// since the previous refresh, so a fresh `Networks` every tick would
// always report zero.
pub fn collect_interfaces(networks: &mut sysinfo::Networks) -> Vec<NetworkInterfaceInfo> {
    networks.refresh(true);
    let hardware_ports = hardware_port_names();

    let mut interfaces: Vec<NetworkInterfaceInfo> = networks
        .list()
        .iter()
        .filter(|(_, data)| is_interesting(data))
        .map(|(name, data)| NetworkInterfaceInfo {
            name: name.clone(),
            display_name: display_name(name, &hardware_ports),
            download_bytes_per_tick: data.received(),
            upload_bytes_per_tick: data.transmitted(),
            packet_errors: data.errors_on_received() + data.errors_on_transmitted(),
            ip_addresses: data
                .ip_networks()
                .iter()
                .map(|ip| ip.addr.to_string())
                .collect(),
        })
        .collect();

    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

// System-wide network traffic, summed across every process — an
// alternative source to collect_interfaces()/sysinfo above. sysinfo's
// per-interface counters mix together every process's traffic, so a burst
// from an unrelated background process (a big upload, a sync job) can
// swamp what a specific app like a browser is actually doing; `nettop`'s
// per-process breakdown lets us sum only real traffic across processes
// the same way, but the numbers come from the same underlying kernel
// counters either way — this doesn't attribute traffic to any one
// process, it's the same system total computed via a different tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemTraffic {
    pub total_bytes_in: u64,
    pub total_bytes_out: u64,
}

// `nettop -x -l 1 -P` prints one process per line as "<name>.<pid>  <in>
// <out>" once given enough terminal width to avoid wrapping (hence
// COLUMNS=), plus a header line and possibly blank lines. Parsing takes
// the trailing two whitespace-separated fields as the byte counts
// regardless of spaces in the process name (e.g. "Google Chrome H.1530"),
// rather than splitting on the first token, which would break on names
// containing spaces.
fn parse_nettop_process_totals(output: &str) -> SystemTraffic {
    let mut total_bytes_in = 0u64;
    let mut total_bytes_out = 0u64;
    for line in output.lines() {
        let mut fields = line.split_whitespace().rev();
        let Some(bytes_out) = fields.next().and_then(|f| f.parse::<u64>().ok()) else {
            continue;
        };
        let Some(bytes_in) = fields.next().and_then(|f| f.parse::<u64>().ok()) else {
            continue;
        };
        // A bare header/blank line has no numeric fields at all and was
        // already skipped above; this guards against a line that
        // coincidentally parses as two numbers but isn't a process row.
        if fields.next().is_none() {
            continue;
        }
        total_bytes_in += bytes_in;
        total_bytes_out += bytes_out;
    }
    SystemTraffic {
        total_bytes_in,
        total_bytes_out,
    }
}

// Takes ~5s regardless of the requested sample count — nettop always
// waits out at least one full sampling interval before printing, even
// with `-l 1` — so callers must run this off the main thread (see
// NetworkTrafficWorker) rather than on a short UI timer.
pub fn collect_system_traffic() -> Option<SystemTraffic> {
    let output = Command::new("nettop")
        .env("COLUMNS", "500")
        .args(["-x", "-l", "1", "-J", "bytes_in,bytes_out", "-P"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(parse_nettop_process_totals(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_errors_is_stable() {
        assert_eq!(classify_stability(0), Stability::Stable);
    }

    #[test]
    fn any_errors_is_unstable() {
        assert_eq!(classify_stability(1), Stability::Unstable);
        assert_eq!(classify_stability(42), Stability::Unstable);
    }

    #[test]
    fn stability_label_text() {
        assert_eq!(Stability::Stable.label(), "Stable");
        assert_eq!(Stability::Unstable.label(), "Unstable");
    }

    fn iface(name: &str, down: u64, up: u64, ips: &[&str]) -> NetworkInterfaceInfo {
        NetworkInterfaceInfo {
            name: name.to_string(),
            display_name: name.to_string(),
            download_bytes_per_tick: down,
            upload_bytes_per_tick: up,
            packet_errors: 0,
            ip_addresses: ips.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn display_name_uses_hardware_port_when_known() {
        let mut ports = HashMap::new();
        ports.insert("en0".to_string(), "Wi-Fi".to_string());
        assert_eq!(display_name("en0", &ports), "Wi-Fi (en0)");
    }

    #[test]
    fn display_name_falls_back_to_well_known_label() {
        let ports = HashMap::new();
        assert_eq!(display_name("utun0", &ports), "VPN/Tunnel (utun0)");
        assert_eq!(display_name("lo0", &ports), "Loopback (lo0)");
        assert_eq!(display_name("awdl0", &ports), "AirDrop/Handoff (awdl0)");
    }

    #[test]
    fn display_name_falls_back_to_bare_name_when_unrecognized() {
        let ports = HashMap::new();
        assert_eq!(display_name("en99", &ports), "en99");
    }

    #[test]
    fn hardware_port_takes_priority_over_well_known_label() {
        // en0 would otherwise fall through unmatched by well_known_label,
        // but if some future device name collided with both, the real
        // per-machine hardware port name should win over a generic guess.
        let mut ports = HashMap::new();
        ports.insert("utun0".to_string(), "Custom VPN Adapter".to_string());
        assert_eq!(display_name("utun0", &ports), "Custom VPN Adapter (utun0)");
    }

    #[test]
    fn select_primary_picks_busiest_interface() {
        let interfaces = vec![
            iface("en0", 1000, 500, &["192.168.1.5"]),
            iface("utun0", 50, 10, &["10.0.0.2"]),
        ];
        assert_eq!(select_primary(&interfaces).unwrap().name, "en0");
    }

    #[test]
    fn select_primary_falls_back_to_first_interface_with_ip_when_all_idle() {
        let interfaces = vec![
            iface("lo0", 0, 0, &["127.0.0.1"]),
            iface("en0", 0, 0, &["192.168.1.5"]),
            iface("utun0", 0, 0, &[]),
        ];
        assert_eq!(select_primary(&interfaces).unwrap().name, "en0");
    }

    #[test]
    fn select_primary_none_when_nothing_qualifies() {
        let interfaces = vec![iface("lo0", 0, 0, &["127.0.0.1"])];
        assert!(select_primary(&interfaces).is_none());
    }

    #[test]
    fn select_primary_empty_list() {
        assert!(select_primary(&[]).is_none());
    }

    #[test]
    fn parse_nettop_sums_process_rows() {
        let output = "\
                                                                                            bytes_in       bytes_out
launchd.1                                                                                          0               0
apsd.555                                                                                       33537          182801
Google Chrome H.1530                                                                        18973907          665882
";
        let traffic = parse_nettop_process_totals(output);
        assert_eq!(traffic.total_bytes_in, 33537 + 18973907);
        assert_eq!(traffic.total_bytes_out, 182801 + 665882);
    }

    #[test]
    fn parse_nettop_ignores_blank_and_header_lines() {
        let output = "\n   \nbytes_in       bytes_out\nfoo.1   10   20\n";
        let traffic = parse_nettop_process_totals(output);
        assert_eq!(traffic.total_bytes_in, 10);
        assert_eq!(traffic.total_bytes_out, 20);
    }

    #[test]
    fn parse_nettop_empty_output() {
        assert_eq!(parse_nettop_process_totals(""), SystemTraffic::default());
    }
}
