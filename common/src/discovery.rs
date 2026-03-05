//! mDNS-SD (multicast DNS Service Discovery) for Gaia Light nodes.
//!
//! Each service (capture, processing, web) registers itself on the local
//! network with a sequential instance name like `capture-01`.

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{debug, info, warn};

const DISCOVERY_SCAN: Duration = Duration::from_secs(3);

// ── Service roles ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRole {
    Capture,
    Processing,
    Web,
}

impl ServiceRole {
    /// mDNS service-type string (≤ 15 bytes per RFC 6763).
    pub fn service_type(&self) -> &'static str {
        match self {
            Self::Capture    => "_gaia-lt-cap._tcp.local.",
            Self::Processing => "_gaia-lt-proc._tcp.local.",
            Self::Web        => "_gaia-lt-web._tcp.local.",
        }
    }

    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Capture    => "capture",
            Self::Processing => "processing",
            Self::Web        => "web",
        }
    }
}

// ── Peer ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Peer {
    pub instance_name: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
}

impl Peer {
    pub fn http_url(&self) -> Option<String> {
        let addr = self
            .addresses
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| self.addresses.first())?;

        Some(match addr {
            IpAddr::V4(v4) => format!("http://{}:{}", v4, self.port),
            IpAddr::V6(v6) => format!("http://[{}]:{}", v6, self.port),
        })
    }

    pub fn non_loopback_addresses(&self) -> Vec<IpAddr> {
        let mut addrs: Vec<IpAddr> = self
            .addresses
            .iter()
            .filter(|a| !a.is_loopback())
            .copied()
            .collect();
        addrs.sort_by_key(|a| !a.is_ipv4());
        addrs
    }
}

// ── Discovery handle ─────────────────────────────────────────────────────────

pub struct DiscoveryHandle {
    daemon: ServiceDaemon,
    instance_name: String,
    fullname: String,
}

impl DiscoveryHandle {
    pub fn instance_name(&self) -> &str {
        &self.instance_name
    }

    pub fn discover_peers(&self, role: ServiceRole, timeout: Duration) -> Vec<Peer> {
        let receiver = match self.daemon.browse(role.service_type()) {
            Ok(r) => r,
            Err(e) => {
                warn!("mDNS browse for {} failed: {e}", role.service_type());
                return vec![];
            }
        };

        debug!(
            "mDNS: browsing for {} (timeout={}s)",
            role.service_type(),
            timeout.as_secs()
        );
        let mut peer_map: HashMap<String, Peer> = HashMap::new();
        let deadline = Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let name = info.get_fullname().to_string();
                    if name == self.fullname {
                        continue;
                    }
                    let addrs: Vec<IpAddr> =
                        info.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
                    let port = info.get_port();
                    let instance = extract_instance_name(&name);

                    let peer = peer_map.entry(instance.clone()).or_insert_with(|| Peer {
                        instance_name: instance,
                        addresses: Vec::new(),
                        port,
                    });
                    for addr in addrs {
                        if !peer.addresses.contains(&addr) {
                            peer.addresses.push(addr);
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        let _ = self.daemon.stop_browse(role.service_type());

        let peers: Vec<Peer> = peer_map.into_values().collect();
        for p in &peers {
            info!(
                "mDNS: peer {} at {:?}:{}",
                p.instance_name, p.addresses, p.port
            );
        }
        peers
    }

    pub fn shutdown(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn register(role: ServiceRole, port: u16) -> Result<DiscoveryHandle> {
    let daemon = ServiceDaemon::new().context("Cannot start mDNS daemon")?;

    let receiver = daemon
        .browse(role.service_type())
        .context("Cannot browse mDNS")?;

    let mut existing: BTreeSet<u32> = BTreeSet::new();
    let deadline = Instant::now() + DISCOVERY_SCAN;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if let Some(n) = parse_instance_number(info.get_fullname(), role.prefix()) {
                    existing.insert(n);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let _ = daemon.stop_browse(role.service_type());

    let our_number = next_available(&existing);
    let instance_name = format!("{}-{:02}", role.prefix(), our_number);
    let host = format!("{}.local.", instance_name);

    let service_info = ServiceInfo::new(
        role.service_type(),
        &instance_name,
        &host,
        "",
        port,
        None,
    )
    .context("Cannot create mDNS ServiceInfo")?
    .enable_addr_auto();

    let fullname = service_info.get_fullname().to_string();
    let registered_addrs = format!("{:?}", service_info.get_addresses());

    daemon
        .register(service_info)
        .context("Cannot register mDNS service")?;

    info!(
        "Registered on mDNS as '{}' (type={}, port={}, addrs={})",
        instance_name,
        role.service_type(),
        port,
        registered_addrs
    );

    Ok(DiscoveryHandle {
        daemon,
        instance_name,
        fullname,
    })
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn parse_instance_number(fullname: &str, prefix: &str) -> Option<u32> {
    let instance = fullname.split('.').next()?;
    let suffix = instance.strip_prefix(prefix)?.strip_prefix('-')?;
    suffix.parse().ok()
}

fn extract_instance_name(fullname: &str) -> String {
    fullname.split('.').next().unwrap_or(fullname).to_string()
}

fn next_available(used: &BTreeSet<u32>) -> u32 {
    let mut n = 1;
    while used.contains(&n) {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_available() {
        let empty = BTreeSet::new();
        assert_eq!(next_available(&empty), 1);

        let set: BTreeSet<u32> = [1, 2, 3].into();
        assert_eq!(next_available(&set), 4);

        let gap: BTreeSet<u32> = [1, 3].into();
        assert_eq!(next_available(&gap), 2);
    }
}
