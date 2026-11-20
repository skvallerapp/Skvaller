//! Fetching addresses that come from feed content: Reader View pages, thumbnails and
//! feed icons.
//!
//! A feed can name any address, such as `http://192.168.1.1/…` or
//! `http://169.254.169.254/…`, and the app would send a request there from the user's
//! machine. [`Untrusted`] reaches only public addresses. It checks the addresses a name
//! resolves to, not its text, on every connection including redirects, so a public name
//! that resolves to a local address is refused too, and nothing can change between the
//! check and the connection.
//!
//! The sync server and the feeds the user subscribed to are not fetched through it:
//! those addresses are the user's choice, and often on the local network. Feed icons
//! trust them too, by host and port (see [`Untrusted::new`]).
//!
//! It never goes through a proxy: a proxy resolves the name itself, out of the guard's
//! sight. Where only a proxy reaches the internet, Reader View, thumbnails and icons
//! fail; syncing and fetching feeds still use it.

use std::error::Error as StdError;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use encoding_rs::Encoding;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use reqwest::{RequestBuilder, Response, Url};

/// Browser-like, since some sites turn away anything else, but says what it is.
pub(crate) const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; Skvaller/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/skvallerapp/Skvaller)"
);

/// Redirects followed at most, as reqwest's default policy does.
const MAX_REDIRECTS: usize = 10;

/// A client for addresses from feed content, which reaches only public addresses.
#[derive(Clone, Debug)]
pub(crate) struct Untrusted {
    http: reqwest::Client,
    guard: Guard,
}

impl Untrusted {
    /// A client whose requests time out after `timeout`. The hosts and ports of
    /// `trusted`, addresses the user chose such as the sync server's, may be reached
    /// at any address.
    pub(crate) fn new(timeout: Duration, trusted: &[Url]) -> reqwest::Result<Self> {
        // The tests' servers listen on loopback; nothing else is let through.
        let ips = if cfg!(test) {
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        } else {
            Vec::new()
        };
        Self::build(timeout, trusted, ips)
    }

    /// As [`Untrusted::new`], but `ips` are reachable besides public addresses. Tests
    /// pass none to check the guard against a local server.
    pub(crate) fn build(
        timeout: Duration,
        trusted: &[Url],
        ips: Vec<IpAddr>,
    ) -> reqwest::Result<Self> {
        let guard = Guard {
            trusted: trusted
                .iter()
                .filter_map(|url| Some((url.host_str()?.to_owned(), url.port_or_known_default())))
                .collect(),
            ips: ips.into(),
        };
        let redirects = guard.clone();
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .no_proxy()
            .dns_resolver(guard.clone())
            // A name is checked as it resolves; an address in the URL never resolves.
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if redirects.allows(attempt.url()) {
                    attempt.follow()
                } else {
                    let to = attempt.url().to_string();
                    attempt.error(Blocked(to))
                }
            }))
            .build()?;
        Ok(Self { http, guard })
    }

    /// A GET of `url`, or `None` when its address is local or private.
    pub(crate) fn get(&self, url: Url) -> Option<RequestBuilder> {
        // reqwest connects to an address in the URL without resolving it.
        self.guard.allows(&url).then(|| self.http.get(url))
    }
}

/// What the article's own document may load: its images, styles and fonts, as Blitz
/// fetches them, not core.
///
/// Public addresses only, with no exceptions, as thumbnails from the same HTML; and
/// only `http`, `https` and `data:` URLs, so no `file:` reads a local file or, on
/// Windows, opens a network share.
#[derive(Clone, Debug)]
pub struct ArticleNet(Untrusted);

impl ArticleNet {
    /// Requests time out after this long, as an image that slow is not coming.
    const TIMEOUT: Duration = Duration::from_secs(30);

    /// Builds the article's client and its URL check.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client can't be built, such as without TLS support.
    pub fn new() -> reqwest::Result<Self> {
        Untrusted::build(Self::TIMEOUT, &[], Vec::new()).map(Self)
    }

    /// The client for the document's requests. Its resolver refuses local and
    /// private addresses, of names and redirects alike; [`ArticleNet::allows`] checks
    /// an address in the URL, which reqwest connects to without resolving.
    #[must_use]
    pub fn client(&self) -> reqwest::Client {
        self.0.http.clone()
    }

    /// Whether the document may request `url`.
    #[must_use]
    pub fn allows(&self, url: &Url) -> bool {
        match url.scheme() {
            "data" => true,
            "http" | "https" => self.0.guard.allows(url),
            _ => false,
        }
    }
}

/// Where an [`Untrusted`] client may connect: public addresses, and its exceptions.
#[derive(Clone, Debug)]
struct Guard {
    /// Hosts and ports the user chose, reachable at any address.
    trusted: Arc<[(String, Option<u16>)]>,
    /// Addresses reachable besides public ones: the tests' servers.
    ips: Arc<[IpAddr]>,
}

impl Guard {
    fn allows_ip(&self, ip: IpAddr) -> bool {
        is_public(ip) || self.ips.contains(&ip)
    }

    /// Whether `host` is a trusted one, on any port: [`Guard::allows`] has checked the
    /// port by the time it resolves.
    fn trusts(&self, host: &str) -> bool {
        self.trusted.iter().any(|(h, _)| h == host)
    }

    /// Whether `url` may be requested. A name is allowed here and checked as it
    /// resolves.
    fn allows(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        // A trusted host only on its own port: its other ports may be anything.
        if self.trusts(host) {
            let port = url.port_or_known_default();
            return self.trusted.iter().any(|(h, p)| h == host && *p == port);
        }
        // An IPv6 address comes in brackets.
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        bare.parse::<IpAddr>().map_or(true, |ip| self.allows_ip(ip))
    }
}

impl Resolve for Guard {
    fn resolve(&self, name: Name) -> Resolving {
        let guard = self.clone();
        Box::pin(async move {
            let host = name.as_str();
            let addrs = tokio::net::lookup_host((host, 0)).await?;
            let trusted = guard.trusts(host);
            let allowed: Vec<SocketAddr> = addrs
                .filter(|a| trusted || guard.allows_ip(a.ip()))
                .collect();
            if allowed.is_empty() {
                return Err(Blocked(host.to_owned()).into());
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// A request refused because its address is local or private.
#[derive(Debug)]
pub(crate) struct Blocked(String);

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is on this device or a private network, which feed content may not reach",
            self.0
        )
    }
}

impl StdError for Blocked {}

/// Whether `e` failed because the guard refused an address, a name's or a redirect's.
pub(crate) fn is_blocked(e: &reqwest::Error) -> bool {
    let mut cause: Option<&(dyn StdError + 'static)> = Some(e);
    while let Some(c) = cause {
        if c.is::<Blocked>() {
            return true;
        }
        cause = c.source();
    }
    false
}

/// Whether `ip` is reachable on the internet at large: not loopback, private,
/// link-local, unique or site local, shared (CGNAT), multicast, reserved or
/// unspecified, nor an IPv6 form that carries such an IPv4 address.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match embedded_v4(v6) {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        // 0.0.0.0/8, "this network", reaches the device itself.
        || a == 0
        // 240.0.0.0/4, reserved, with the broadcast address.
        || a >= 240
        // 100.64.0.0/10, shared address space for carrier-grade NAT.
        || (a == 100 && b & 0xc0 == 64)
        // 192.0.0.0/24, protocol assignments; 198.18.0.0/15, benchmarking.
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && b & 0xfe == 18))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
        // fec0::/10, site local, deprecated but still routed on some networks.
        || first & 0xffc0 == 0xfec0
        // 64:ff9b:1::/48, NAT64 for local use.
        || ip.segments()[..3] == [0x64, 0xff9b, 1])
}

/// The IPv4 address an IPv6 one reaches: IPv4-mapped, IPv4-compatible, NAT64
/// (`64:ff9b::/96`) and 6to4 (`2002::/16`).
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| Some(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
    match s {
        [0, 0, 0, 0, 0, 0xffff, hi, lo]
        | [0x64, 0xff9b, 0, 0, 0, 0, hi, lo]
        | [0x2002, hi, lo, ..] => v4(hi, lo),
        // :: and ::1 are IPv6's own.
        [0, 0, 0, 0, 0, 0, hi, lo] if (hi, lo) > (0, 1) => v4(hi, lo),
        _ => None,
    }
}

/// Only http(s) addresses: any other scheme from untrusted content is refused.
pub(crate) fn web_url(url: Url) -> Option<Url> {
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// `url`, trimmed, made absolute against `base`, or parsed as it is without one.
pub(crate) fn absolute(base: Option<&Url>, url: &str) -> Option<Url> {
    let url = url.trim();
    match base {
        Some(base) => base.join(url).ok(),
        None => Url::parse(url).ok(),
    }
}

/// The body of `res`, or `Ok(None)` if it is larger than `max` bytes. Reads up to the
/// limit only: a server may send no length, or the wrong one.
pub(crate) async fn read_capped(mut res: Response, max: usize) -> reqwest::Result<Option<Vec<u8>>> {
    if res.content_length().is_some_and(|n| n > max as u64) {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > max {
            return Ok(None);
        }
    }
    Ok(Some(body))
}

/// Decodes like a browser, mostly: a byte order mark, else the Content-Type charset,
/// else a `<meta>` charset in the first 1024 bytes, else UTF-8.
pub(crate) fn decode(body: &[u8], content_type: &str) -> String {
    let encoding = content_type
        .split(';')
        .skip(1)
        .find_map(|param| {
            let (name, value) = param.split_once('=')?;
            (name.trim() == "charset").then(|| label(value.trim().trim_matches(['"', '\''])))?
        })
        .or_else(|| meta_charset(&body[..body.len().min(1024)]))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(body).0.into_owned()
}

/// The charset of a `<meta charset>` or `<meta http-equiv="Content-Type">` tag. A rough
/// search for `charset=`, not the HTML spec's full prescan.
fn meta_charset(head: &[u8]) -> Option<&'static Encoding> {
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    let mut rest = head.as_str();
    while let Some(i) = rest.find("charset") {
        rest = &rest[i + "charset".len()..];
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start().trim_start_matches(['"', '\'']);
        let end = value
            .find(|c: char| c.is_whitespace() || "\"';>/".contains(c))
            .unwrap_or(value.len());
        if let Some(encoding) = label(&value[..end]) {
            // UTF-16 becomes UTF-8: a page whose meta tag reads as ASCII isn't UTF-16.
            return Some(encoding.output_encoding());
        }
    }
    None
}

fn label(name: &str) -> Option<&'static Encoding> {
    Encoding::for_label(name.as_bytes())
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn decodes_with_the_header_charset_then_the_meta_charset() {
        assert_eq!(decode(b"caf\xe9", "text/html; charset=ISO-8859-1"), "café");
        assert_eq!(
            decode(b"caf\xe9", "text/html; charset = \"windows-1252\""),
            "café"
        );
        assert_eq!(decode("café".as_bytes(), "text/html"), "café");
        let meta = b"<html><head><meta charset=\"windows-1252\"></head><body>r\xe4ksm\xf6rg\xe5s";
        assert!(decode(meta, "text/html").ends_with("räksmörgås"));
        let http_equiv =
            b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\">\xe5";
        assert!(decode(http_equiv, "text/html").ends_with('å'));
        // The header wins over the meta tag.
        let both = "<meta charset=\"windows-1252\">å".as_bytes();
        assert!(decode(both, "text/html; charset=utf-8").ends_with('å'));
        // A meta tag can't make a page UTF-16.
        assert_eq!(
            meta_charset(b"<meta charset=utf-16>"),
            Some(encoding_rs::UTF_8)
        );
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn only_public_addresses_count_as_public() {
        for local in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.254",
            "0.0.0.0",
            "0.1.2.3",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "::ffff:192.168.1.1",
            "::ffff:127.0.0.1",
            "::7f00:1",
            "64:ff9b::c0a8:101",
            "64:ff9b:1::1",
            "2002:c0a8:101::1",
            "fec0::1",
            "ff02::1",
            "224.0.0.1",
            "240.0.0.1",
            "192.0.0.8",
            "198.18.0.1",
        ] {
            assert!(!is_public(local.parse().unwrap()), "{local}");
        }
        for public in [
            "93.184.216.34",
            "100.128.0.1",
            "8.8.8.8",
            "2606:4700::1111",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn the_article_loads_only_public_web_and_data_urls() {
        let net = ArticleNet::new().unwrap();
        assert!(net.allows(&url("https://example.com/a.png")));
        assert!(net.allows(&url("http://93.184.216.34/a.png")));
        assert!(net.allows(&url("data:image/png;base64,AAAA")));
        for refused in [
            "http://127.0.0.1/a.png",
            "http://192.168.1.1/admin",
            "https://[::1]/a.png",
            "http://169.254.169.254/latest/meta-data",
            "file:///etc/passwd",
            "file://server/share/a.png",
            "ftp://example.com/a.png",
            "dioxus://index.html/a.png",
        ] {
            assert!(!net.allows(&url(refused)), "{refused}");
        }
    }

    #[tokio::test]
    async fn the_articles_client_refuses_a_name_for_a_local_address() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let net = ArticleNet::new().unwrap();
        let named = format!("http://localhost:{}/a.png", server.address().port());

        let e = net.client().get(&named).send().await.unwrap_err();

        assert!(is_blocked(&e), "{e}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn a_local_address_in_the_url_is_refused_before_any_request() {
        let http = Untrusted::build(TIMEOUT, &[], Vec::new()).unwrap();
        assert!(http.get(url("http://127.0.0.1:9/")).is_none());
        assert!(http.get(url("http://[::ffff:192.168.0.1]/")).is_none());
        assert!(http.get(url("http://169.254.169.254/latest")).is_none());
        // A name waits for its addresses.
        assert!(http.get(url("http://example.com/")).is_some());
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_a_local_address_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let port = server.address().port();
        let http = Untrusted::build(TIMEOUT, &[], Vec::new()).unwrap();
        let e = http
            .get(url(&format!("http://localhost:{port}/")))
            .unwrap()
            .send()
            .await
            .unwrap_err();
        assert!(is_blocked(&e), "{e:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_redirect_to_a_local_address_is_refused() {
        // 127.0.0.1 stands in for a public server here; 127.0.0.2 is local to it.
        let server = MockServer::start().await;
        let port = server.address().port();
        Mock::given(path("/moved"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("http://127.0.0.2:{port}/admin")),
            )
            .mount(&server)
            .await;
        let http = Untrusted::build(TIMEOUT, &[], vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
        let e = http
            .get(url(&format!("http://127.0.0.1:{port}/moved")))
            .unwrap()
            .send()
            .await
            .unwrap_err();
        assert!(e.is_redirect() && is_blocked(&e), "{e:?}");
    }

    #[tokio::test]
    async fn an_allowed_address_and_the_trusted_host_are_fetched() {
        let server = MockServer::start().await;
        Mock::given(path("/page"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let port = server.address().port();
        let allowed =
            Untrusted::build(TIMEOUT, &[], vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
        let res = allowed
            .get(url(&format!("http://127.0.0.1:{port}/page")))
            .unwrap()
            .send()
            .await
            .unwrap();
        assert_eq!(res.text().await.unwrap(), "ok");
        // The sync server, by name, though it resolves to loopback; on its port only.
        let server_url = url(&format!("http://localhost:{port}/"));
        let trusted = Untrusted::build(TIMEOUT, &[server_url], Vec::new()).unwrap();
        let res = trusted
            .get(url(&format!("http://localhost:{port}/page")))
            .unwrap()
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success());
        assert!(trusted.get(url("http://localhost:22/")).is_none());
    }
}
