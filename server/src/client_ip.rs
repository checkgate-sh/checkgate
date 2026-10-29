//! Resolves the client IP address a request is attributed to.
//!
//! The TCP peer address is the only value a client cannot choose, so it is the
//! default. `X-Forwarded-For` is consulted only when that peer is a proxy the
//! operator listed in `TRUSTED_PROXIES`; otherwise any client could pick its
//! own rate-limit identity (or frame another address) by sending the header.
//!
//! Proxies append the address they received a connection from, so the chain is
//! read from the right: each hop that is itself a trusted proxy is skipped, and
//! the first untrusted hop is the client. Entries left of that hop were written
//! by the client and are never used.

use axum::{
    extract::{ConnectInfo, FromRequestParts, State},
    http::{HeaderMap, Request, StatusCode, request::Parts},
    middleware::Next,
    response::Response,
};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// Upper bound on the hops examined, so a long header costs bounded work.
const MAX_HOPS: usize = 32;

/// Which forwarding header trusted proxies set. Exactly one is read: a proxy
/// that writes one header usually passes the other through from the client
/// untouched, so honouring both would let clients pick their address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForwardingHeader {
    /// `X-Forwarded-For: client, proxy1` (the de-facto standard; default).
    #[default]
    XForwardedFor,
    /// RFC 7239 `Forwarded: for=client;proto=https, for="[2001:db8::1]:443"`.
    Forwarded,
}

/// Proxies whose forwarding header is believed.
#[derive(Clone, Debug, Default)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
    header: ForwardingHeader,
}

impl TrustedProxies {
    /// Parses a comma- or whitespace-separated list of IP addresses and CIDR
    /// ranges, e.g. `"10.0.0.0/8, 192.168.1.10, fd00::/8"`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let nets = spec
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                entry
                    .parse::<IpNet>()
                    .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                    .map(|net| net.trunc())
                    .map_err(|_| format!("invalid TRUSTED_PROXIES entry {entry:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            nets,
            header: ForwardingHeader::default(),
        })
    }

    pub fn with_header(mut self, header: ForwardingHeader) -> Self {
        self.header = header;
        self
    }

    /// Reads `TRUSTED_PROXIES` (unset or empty: no proxy is trusted) and
    /// `TRUSTED_PROXY_HEADER` (`x-forwarded-for`, the default, or `forwarded`).
    pub fn from_env() -> Result<Self, String> {
        let header = match std::env::var("TRUSTED_PROXY_HEADER")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "x-forwarded-for" => ForwardingHeader::XForwardedFor,
            "forwarded" => ForwardingHeader::Forwarded,
            other => {
                return Err(format!(
                    "TRUSTED_PROXY_HEADER must be x-forwarded-for or forwarded, got {other:?}"
                ));
            }
        };
        Ok(Self::parse(&std::env::var("TRUSTED_PROXIES").unwrap_or_default())?.with_header(header))
    }

    pub fn header(&self) -> ForwardingHeader {
        self.header
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.nets.iter().any(|net| net.contains(&ip))
    }

    /// The address the request is attributed to.
    pub fn resolve(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let peer = peer.to_canonical();
        if !self.is_trusted(peer) {
            return peer;
        }
        let name = match self.header {
            ForwardingHeader::XForwardedFor => "x-forwarded-for",
            ForwardingHeader::Forwarded => "forwarded",
        };
        let mut hops = Vec::new();
        for value in headers.get_all(name) {
            // A header that is not visible ASCII cannot be a list of addresses;
            // ignore the whole chain rather than guess which part is genuine.
            let Ok(value) = value.to_str() else {
                return peer;
            };
            hops.extend(value.split(',').map(|element| match self.header {
                ForwardingHeader::XForwardedFor => element.trim(),
                ForwardingHeader::Forwarded => forwarded_for(element),
            }));
        }
        let mut client = peer;
        for hop in hops.iter().rev().take(MAX_HOPS) {
            // A malformed hop ends the walk. Everything to its right was
            // written by trusted proxies, so the nearest of those stands in for
            // the client: a broken header can only merge clients into a
            // proxy's budget, never hand a client a fresh one.
            let Some(ip) = parse_hop(hop) else {
                return client;
            };
            client = ip;
            if !self.is_trusted(ip) {
                return ip;
            }
        }
        client
    }
}

/// The `for=` value of one RFC 7239 `Forwarded` element, unquoted; empty when
/// absent. Obfuscated identifiers (`_hidden`) and `unknown` are returned as
/// they are and then fail to parse as an address, which ends the walk at the
/// nearest trusted proxy — as does a comma inside a quoted value, which
/// splitting the list breaks into malformed elements.
fn forwarded_for(element: &str) -> &str {
    element
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("for"))
        .map(|(_, value)| {
            let value = value.trim();
            value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value)
        })
        .unwrap_or("")
}

/// Parses one `X-Forwarded-For` entry (or `Forwarded` `for=` value): a bare address, or an address with a
/// port (`203.0.113.7:4711`, `[2001:db8::1]:443`), which some proxies emit.
fn parse_hop(hop: &str) -> Option<IpAddr> {
    let ip = hop
        .parse::<IpAddr>()
        .ok()
        .or_else(|| hop.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
        .or_else(|| {
            hop.strip_prefix('[')
                .and_then(|h| h.strip_suffix(']'))
                .and_then(|h| h.parse::<IpAddr>().ok())
        })?;
    Some(ip.to_canonical())
}

/// The resolved client address, placed in request extensions by
/// [`resolve_client_ip`]. Handlers extract it instead of `ConnectInfo` so that
/// logs, lockout records and the SDK health view agree with the rate limiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(ip) = parts.extensions.get::<ClientIp>() {
            return Ok(*ip);
        }
        // Without the middleware, the TCP peer is still the safe answer.
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| ClientIp(addr.ip().to_canonical()))
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Middleware that resolves the client address once per request.
pub async fn resolve_client_ip(
    State(trusted): State<Arc<TrustedProxies>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let ip = trusted.resolve(addr.ip(), req.headers());
    req.extensions_mut().insert(ClientIp(ip));
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn xff(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for v in values {
            headers.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        headers
    }

    #[test]
    fn direct_request_ignores_spoofed_header() {
        let trusted = TrustedProxies::default();
        let headers = xff(&["198.51.100.1"]);
        assert_eq!(
            trusted.resolve(ip("203.0.113.9"), &headers),
            ip("203.0.113.9")
        );
    }

    #[test]
    fn untrusted_proxy_header_is_ignored() {
        let trusted = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let headers = xff(&["198.51.100.1"]);
        // The peer is not in the trusted range, so it is the client.
        assert_eq!(trusted.resolve(ip("192.0.2.5"), &headers), ip("192.0.2.5"));
    }

    #[test]
    fn trusted_proxy_forwards_client() {
        let trusted = TrustedProxies::parse("10.0.0.1").unwrap();
        let headers = xff(&["198.51.100.1"]);
        assert_eq!(
            trusted.resolve(ip("10.0.0.1"), &headers),
            ip("198.51.100.1")
        );
    }

    #[test]
    fn trusted_proxy_without_header_is_the_client() {
        let trusted = TrustedProxies::parse("10.0.0.1").unwrap();
        assert_eq!(
            trusted.resolve(ip("10.0.0.1"), &HeaderMap::new()),
            ip("10.0.0.1")
        );
    }

    #[test]
    fn multi_hop_chain_takes_rightmost_untrusted() {
        let trusted = TrustedProxies::parse("10.0.0.0/8, 172.16.0.0/12").unwrap();
        // Client spoofs 1.1.1.1; the edge proxy (172.16.0.2) appended the real
        // client 198.51.100.7; the inner proxy appended the edge proxy.
        let headers = xff(&["1.1.1.1, 198.51.100.7, 172.16.0.2"]);
        assert_eq!(
            trusted.resolve(ip("10.0.0.1"), &headers),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn chain_split_across_header_lines_is_joined_in_order() {
        let trusted = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let headers = xff(&["1.1.1.1", "198.51.100.7, 10.0.0.2"]);
        assert_eq!(
            trusted.resolve(ip("10.0.0.1"), &headers),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn all_trusted_chain_falls_back_to_leftmost_hop() {
        let trusted = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let headers = xff(&["10.0.0.3, 10.0.0.2"]);
        assert_eq!(trusted.resolve(ip("10.0.0.1"), &headers), ip("10.0.0.3"));
    }

    #[test]
    fn malformed_hop_stops_at_nearest_trusted_address() {
        let trusted = TrustedProxies::parse("10.0.0.0/8").unwrap();
        for bad in ["not-an-ip", "", "unknown", "999.1.1.1", "1.2.3.4/24"] {
            let headers = xff(&[&format!("198.51.100.7, {bad}")]);
            assert_eq!(
                trusted.resolve(ip("10.0.0.1"), &headers),
                ip("10.0.0.1"),
                "hop {bad:?}"
            );
            let headers = xff(&[&format!("{bad}, 10.0.0.2")]);
            assert_eq!(
                trusted.resolve(ip("10.0.0.1"), &headers),
                ip("10.0.0.2"),
                "hop {bad:?} left of a trusted hop"
            );
        }
    }

    #[test]
    fn non_ascii_header_is_ignored() {
        let trusted = TrustedProxies::parse("10.0.0.1").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_bytes(b"198.51.100.1\xff").unwrap(),
        );
        assert_eq!(trusted.resolve(ip("10.0.0.1"), &headers), ip("10.0.0.1"));
    }

    #[test]
    fn hops_with_ports_and_brackets_parse() {
        let trusted = TrustedProxies::parse("10.0.0.1").unwrap();
        for (hop, want) in [
            ("198.51.100.1:4711", "198.51.100.1"),
            ("[2001:db8::1]:443", "2001:db8::1"),
            ("[2001:db8::2]", "2001:db8::2"),
            ("2001:db8::3", "2001:db8::3"),
        ] {
            assert_eq!(
                trusted.resolve(ip("10.0.0.1"), &xff(&[hop])),
                ip(want),
                "{hop}"
            );
        }
    }

    #[test]
    fn ipv4_mapped_addresses_are_canonical() {
        let trusted = TrustedProxies::parse("10.0.0.1").unwrap();
        let headers = xff(&["::ffff:198.51.100.1"]);
        assert_eq!(
            trusted.resolve(ip("::ffff:10.0.0.1"), &headers),
            ip("198.51.100.1")
        );
    }

    fn fwd(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for v in values {
            headers.append("forwarded", HeaderValue::from_str(v).unwrap());
        }
        headers
    }

    #[test]
    fn rfc7239_forwarded_is_read_only_when_selected() {
        let proxies = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let rfc = proxies.clone().with_header(ForwardingHeader::Forwarded);
        let h = fwd(&["for=198.51.100.7;proto=https, for=10.0.0.2"]);
        assert_eq!(rfc.resolve(ip("10.0.0.1"), &h), ip("198.51.100.7"));
        // Default mode ignores Forwarded entirely.
        assert_eq!(proxies.resolve(ip("10.0.0.1"), &h), ip("10.0.0.1"));
        // Forwarded mode ignores X-Forwarded-For, which a proxy may pass through.
        assert_eq!(
            rfc.resolve(ip("10.0.0.1"), &xff(&["203.0.113.9"])),
            ip("10.0.0.1")
        );
    }

    #[test]
    fn rfc7239_forms_parse() {
        let rfc = TrustedProxies::parse("10.0.0.0/8")
            .unwrap()
            .with_header(ForwardingHeader::Forwarded);
        for (header, want) in [
            (r#"For="[2001:db8:cafe::17]:4711""#, "2001:db8:cafe::17"),
            ("for=192.0.2.60;proto=http;by=203.0.113.43", "192.0.2.60"),
            (r#"proto=https; for="198.51.100.1:8443""#, "198.51.100.1"),
            ("for=203.0.113.5, for=10.1.1.1", "203.0.113.5"),
        ] {
            assert_eq!(
                rfc.resolve(ip("10.0.0.1"), &fwd(&[header])),
                ip(want),
                "{header}"
            );
        }
        // Spoofed entries left of an untrusted hop are never used.
        let h = fwd(&["for=1.1.1.1, for=203.0.113.5, for=10.1.1.1"]);
        assert_eq!(rfc.resolve(ip("10.0.0.1"), &h), ip("203.0.113.5"));
        // Obfuscated, unknown, missing or broken values stop at the nearest
        // trusted proxy instead of handing out a fresh identity.
        for header in [
            "for=_hidden, for=10.1.1.1",
            "for=unknown, for=10.1.1.1",
            "proto=https, for=10.1.1.1",
            r#"for="[2001:db8::1,]", for=10.1.1.1"#,
        ] {
            assert_eq!(
                rfc.resolve(ip("10.0.0.1"), &fwd(&[header])),
                ip("10.1.1.1"),
                "{header}"
            );
        }
    }

    #[test]
    fn hop_walk_is_bounded() {
        let trusted = TrustedProxies::parse("10.0.0.0/8").unwrap();
        let mut chain = vec!["198.51.100.1".to_string()];
        chain.extend((0..100).map(|i| format!("10.0.{}.{}", i / 250, i % 250 + 1)));
        let headers = xff(&[&chain.join(", ")]);
        // The real client sits beyond MAX_HOPS trusted hops, so the walk stops
        // on a trusted address instead of reaching it.
        assert!(trusted.is_trusted(trusted.resolve(ip("10.0.0.1"), &headers)));
    }

    #[test]
    fn parse_accepts_ips_and_cidrs_and_rejects_garbage() {
        let trusted = TrustedProxies::parse(" 10.0.0.0/8,192.168.1.10  fd00::/8 ").unwrap();
        assert!(trusted.is_trusted(ip("10.255.0.1")));
        assert!(trusted.is_trusted(ip("192.168.1.10")));
        assert!(!trusted.is_trusted(ip("192.168.1.11")));
        assert!(trusted.is_trusted(ip("fd12::1")));
        assert!(TrustedProxies::parse("").unwrap().is_empty());
        assert!(TrustedProxies::parse("10.0.0.0/33").is_err());
        assert!(TrustedProxies::parse("proxy.internal").is_err());
    }
}
