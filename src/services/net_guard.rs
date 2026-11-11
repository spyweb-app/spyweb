use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::OnceLock;

use ureq::Agent;
use ureq::Error;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

pub fn parse_flag(v: Option<&str>) -> bool {
    let Some(v) = v else {
        return false;
    };
    !matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

pub fn block_private() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        let value = std::env::var("SPYWEB_BLOCK_PRIVATE_NETWORKS").ok();
        if let Some(raw) = value.as_deref() {
            let normalized = raw.trim().to_ascii_lowercase();
            if !normalized.is_empty()
                && !matches!(
                    normalized.as_str(),
                    "0" | "1" | "false" | "true" | "no" | "yes" | "off" | "on"
                )
            {
                crate::t_warnln!(
                    "SPYWEB_BLOCK_PRIVATE_NETWORKS has unrecognized value '{}'; enabling private-network blocking",
                    raw
                );
            }
        }
        parse_flag(value.as_deref())
    })
}

pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_forbidden_v4(v4),
        IpAddr::V6(v6) => is_forbidden_v6(v6),
    }
}

fn is_forbidden_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xC0) == 64)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 88 && o[2] == 99)
        || (o[0] == 198 && (o[1] & 0xFE) == 18)
        || o[0] >= 240
}

fn is_forbidden_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_forbidden_v4(v4);
    }
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return true;
    }

    let s = ip.segments();

    if s[..6] == [0u16; 6] {
        return true;
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0u16; 4] {
        return is_forbidden_v4(v4_from(s[6], s[7]));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001 {
        return true;
    }
    if s[0] == 0x0100 && s[1..4] == [0u16; 3] {
        return true;
    }
    if s[0] == 0x2002 {
        return is_forbidden_v4(v4_from(s[1], s[2]));
    }
    if s[0] == 0x2001 && (s[1] == 0x0000 || s[1] == 0x0db8) {
        return true;
    }
    if s[0] == 0x2001 && s[1] == 0x0002 && s[2] == 0x0000 {
        return true;
    }
    if s[0] == 0x2001 && ((s[1] & 0xFFF0) == 0x0010 || (s[1] & 0xFFF0) == 0x0020) {
        return true;
    }
    if s[0] == 0x3fff && (s[1] & 0xF000) == 0 {
        return true;
    }
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0xffff && s[5] == 0 {
        return is_forbidden_v4(v4_from(s[6], s[7]));
    }

    (s[0] & 0xFE00) == 0xFC00 || (s[0] & 0xFFC0) == 0xFE80 || (s[0] & 0xFFC0) == 0xFEC0
}

fn v4_from(hi: u16, lo: u16) -> Ipv4Addr {
    Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
}

fn blocked_msg(ip: IpAddr) -> String {
    format!("blocked private network address: {ip}")
}

pub fn resolve_validated(host: &str, port: u16, enabled: bool) -> Result<Vec<SocketAddr>, String> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("dns resolution failed for {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("no addresses for {host}"));
    }
    if enabled && let Some(addr) = addrs.iter().find(|a| is_forbidden_ip(a.ip())) {
        return Err(blocked_msg(addr.ip()));
    }
    Ok(addrs)
}

#[derive(Debug)]
pub struct GuardedResolver {
    inner: DefaultResolver,
    enabled: bool,
}

impl GuardedResolver {
    pub fn new(enabled: bool) -> Self {
        Self {
            inner: DefaultResolver::default(),
            enabled,
        }
    }
}

impl Resolver for GuardedResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, Error> {
        let addrs = self.inner.resolve(uri, config, timeout)?;
        if self.enabled
            && let Some(addr) = addrs.iter().find(|a| is_forbidden_ip(a.ip()))
        {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                blocked_msg(addr.ip()),
            )));
        }
        Ok(addrs)
    }
}

pub fn agent_with_guard(config: Config) -> Result<Agent, String> {
    agent_with_guard_enabled(config, block_private())
}

fn agent_with_guard_enabled(config: Config, enabled: bool) -> Result<Agent, String> {
    if enabled && config.proxy().is_some() {
        return Err(
            "private-network guard is incompatible with proxies; unset SPYWEB_BLOCK_PRIVATE_NETWORKS or remove the proxy"
                .to_string(),
        );
    }
    Ok(Agent::with_parts(
        config,
        DefaultConnector::new(),
        GuardedResolver::new(enabled),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn parse_flag_accepts_known_values() {
        assert!(!parse_flag(None));
        assert!(!parse_flag(Some("")));
        assert!(!parse_flag(Some("0")));
        assert!(!parse_flag(Some("false")));
        assert!(!parse_flag(Some("off")));
        assert!(!parse_flag(Some("no")));
        assert!(parse_flag(Some("1")));
        assert!(parse_flag(Some("true")));
        assert!(parse_flag(Some("YES")));
        assert!(parse_flag(Some(" on ")));
        assert!(parse_flag(Some("garbage")));
    }

    #[test]
    fn forbidden_ipv4_addresses() {
        for value in [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "192.0.0.1",
            "192.88.99.1",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(is_forbidden_ip(ip(value)), "{value} should be blocked");
        }
    }

    #[test]
    fn forbidden_ipv6_addresses() {
        for value in [
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001::1",
            "fe80::1",
            "fc00::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "2001:2::1",
            "2001:10::1",
            "2001:20::1",
            "3fff::1",
        ] {
            assert!(is_forbidden_ip(ip(value)), "{value} should be blocked");
        }
    }

    #[test]
    fn allowed_public_addresses() {
        for value in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:808:808::",
        ] {
            assert!(!is_forbidden_ip(ip(value)), "{value} should be allowed");
        }
    }

    #[test]
    fn cgnat_and_benchmark_boundaries() {
        assert!(!is_forbidden_ip(ip("100.63.255.255")));
        assert!(is_forbidden_ip(ip("100.64.0.0")));
        assert!(is_forbidden_ip(ip("100.127.255.255")));
        assert!(!is_forbidden_ip(ip("100.128.0.0")));
        assert!(!is_forbidden_ip(ip("198.17.255.255")));
        assert!(is_forbidden_ip(ip("198.19.255.255")));
    }

    #[test]
    fn resolve_validated_blocks_loopback_only_when_enabled() {
        assert!(resolve_validated("127.0.0.1", 80, true).is_err());
        assert!(resolve_validated("127.0.0.1", 80, false).is_ok());
    }

    #[test]
    fn guard_rejects_proxy_configuration() {
        let proxy = ureq::Proxy::new("http://127.0.0.1:8080").unwrap();
        let config = Agent::config_builder().proxy(Some(proxy)).build();
        assert!(agent_with_guard_enabled(config, true).is_err());

        let proxy = ureq::Proxy::new("http://127.0.0.1:8080").unwrap();
        let config = Agent::config_builder().proxy(Some(proxy)).build();
        assert!(agent_with_guard_enabled(config, false).is_ok());
    }

    #[test]
    fn guarded_agent_blocks_loopback_http() {
        let server = crate::services::server::types::MockServer::start(|_| {
            crate::services::server::types::Response::text("ok")
        });
        let config = Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(2)))
            .http_status_as_error(false)
            .build();
        let agent = agent_with_guard_enabled(config, true).unwrap();
        let error = agent
            .get(&format!("http://127.0.0.1:{}/", server.port()))
            .call()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("blocked private network address")
        );
    }
}
