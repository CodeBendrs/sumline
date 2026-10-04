//! Which network addresses count as "our own reverse proxy".
//!
//! A proxy in front of the app (Railway's edge, Caddy) hides the visitor's address behind its
//! own, so rate limits need `X-Forwarded-For`. That header is only believable when it was
//! written by a proxy we run, so trust is configured as address ranges.

use anyhow::{Result, anyhow};
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

/// Loopback and the private, link-local and carrier-grade-NAT ranges. Reverse proxies on a
/// platform's internal network (Railway, Docker, a local Caddy) live in these.
const PRIVATE: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
];

/// An IPv4 address that arrived through a dual-stack socket looks like `::ffff:a.b.c.d`.
fn normalised(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

impl Cidr {
    fn parse_one(text: &str) -> Result<Cidr> {
        let invalid = || {
            anyhow!(
                "INVOICE_TRUSTED_PROXY must list addresses, ranges (10.0.0.0/8) or the word private"
            )
        };
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => {
                (address, Some(prefix.parse::<u8>().map_err(|_| invalid())?))
            }
            None => (text, None),
        };
        let network = normalised(address.trim().parse::<IpAddr>().map_err(|_| invalid())?);
        let widest = if network.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(widest);
        if prefix > widest {
            return Err(invalid());
        }
        Ok(Cidr { network, prefix })
    }

    /// Comma-separated entries: single addresses, CIDR ranges, or the keyword `private`.
    pub fn parse_list(text: &str) -> Result<Vec<Cidr>> {
        let mut found = Vec::new();
        for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if entry.eq_ignore_ascii_case("private") {
                for range in PRIVATE {
                    found.push(Cidr::parse_one(range)?);
                }
            } else {
                found.push(Cidr::parse_one(entry)?);
            }
        }
        Ok(found)
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, normalised(ip)) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                u32::from(network) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                u128::from(network) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }
    fn trusts(list: &str, address: &str) -> bool {
        Cidr::parse_list(list)
            .unwrap()
            .iter()
            .any(|c| c.contains(ip(address)))
    }

    #[test]
    fn ranges_addresses_and_the_private_keyword() {
        assert!(trusts("10.0.0.0/8", "10.200.1.9") && !trusts("10.0.0.0/8", "11.0.0.1"));
        assert!(trusts("127.0.0.1", "127.0.0.1") && !trusts("127.0.0.1", "127.0.0.2"));
        assert!(trusts("100.0.0.0/8", "100.64.1.1") && trusts("100.0.0.0/8", "100.12.1.1"));
        assert!(trusts("fd12::/16", "fd12:1:2::3") && !trusts("fd12::/16", "fd13::1"));
        assert!(
            trusts("0.0.0.0/0", "8.8.8.8"),
            "a zero-length prefix matches everything"
        );
        for inside in [
            "10.1.1.1",
            "172.20.0.5",
            "192.168.1.1",
            "100.64.0.9",
            "127.0.0.1",
            "::1",
            "fd00::5",
        ] {
            assert!(trusts("private", inside), "{inside}");
        }
        for outside in ["8.8.8.8", "203.0.113.9", "172.32.0.1", "2001:db8::1"] {
            assert!(!trusts("private", outside), "{outside}");
        }
        assert!(
            trusts("private, 100.0.0.0/8", "100.1.2.3"),
            "keyword and ranges combine"
        );
        assert!(
            trusts("10.0.0.0/8", "::ffff:10.1.2.3"),
            "IPv4 seen through a dual-stack socket"
        );
    }

    #[test]
    fn nonsense_is_refused_rather_than_silently_trusting_nothing() {
        for bad in [
            "not-an-address",
            "10.0.0.0/33",
            "10.0.0.0/x",
            "::/129",
            "10.0.0.1/8/8",
        ] {
            assert!(Cidr::parse_list(bad).is_err(), "{bad}");
        }
        assert!(Cidr::parse_list("").unwrap().is_empty());
    }
}
