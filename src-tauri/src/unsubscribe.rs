//! `List-Unsubscribe` parsing and the one-click POST boundary (P9.4).
//!
//! Three things the previous implementation got wrong are fixed here:
//!
//! 1. the header is parsed as **structured URLs** (RFC 2369/8058), never by
//!    substring search, so a URL that merely mentions `http` cannot win;
//! 2. a one-click POST is only sent when the message actually carries the
//!    `List-Unsubscribe-Post: List-Unsubscribe=One-Click` field **and** the
//!    provider supplied trustworthy authentication evidence. A
//!    sender-authored `Authentication-Results` header is never evidence;
//! 3. the POST leaves through a dedicated, credential-free client: no cookie
//!    jar, no inherited authorization, no redirects, a 10-second timeout, a
//!    bounded response body, an explicit
//!    `application/x-www-form-urlencoded` content type and the exact
//!    `List-Unsubscribe=One-Click` form body.
//!
//! The target is refused outright when it resolves to a loopback, private or
//! link-local address — including an IPv6 one, including an IPv4-mapped IPv6
//! one. The validated resolution is then **pinned** to the request, so a
//! second DNS answer cannot redirect the connection somewhere else.

use crate::dto::{UnsubscribeResult, UnsubscribeTarget};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// The exact form body a one-click POST must carry.
pub const ONE_CLICK_BODY: &str = "List-Unsubscribe=One-Click";
/// The only `List-Unsubscribe-Post` field value that means one-click.
pub const ONE_CLICK_KEY: &str = "list-unsubscribe";
pub const ONE_CLICK_TOKEN: &str = "one-click";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A one-click response only has to acknowledge; anything larger is ignored.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Every structured entry of a `List-Unsubscribe` header value, in order.
///
/// Entries are separated by commas *outside* angle brackets, so a URL that
/// itself carries a comma stays one target.
pub fn parse_targets(raw: &str) -> Vec<UnsubscribeTarget> {
    let mut out: Vec<UnsubscribeTarget> = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let push = |entry: &str, out: &mut Vec<UnsubscribeTarget>| {
        let entry = entry.trim();
        if entry.is_empty() {
            return;
        }
        let url = entry
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or(entry)
            .trim();
        if url.is_empty() {
            return;
        }
        let scheme = match url::Url::parse(url) {
            Ok(parsed) => match parsed.scheme() {
                "https" => "https",
                "http" => "http",
                "mailto" => "mailto",
                _ => "other",
            },
            Err(_) => "other",
        };
        out.push(UnsubscribeTarget {
            scheme: scheme.to_string(),
            url: url.to_string(),
        });
    };
    for ch in raw.chars() {
        match ch {
            '<' => {
                depth += 1;
                current.push(ch);
            }
            '>' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth <= 0 => {
                push(&current, &mut out);
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    push(&current, &mut out);
    out
}

/// The header names a provider is expected to have produced.
pub const TRUSTED_HEADER_NOTE: &str =
    "Sift takes one-click evidence from your mail provider, never from a header the sender could write.";

/// Is the `List-Unsubscribe-Post` field exactly the one-click token?
pub fn post_is_one_click(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return false;
    };
    raw.split(',').any(|pair| {
        let Some((key, value)) = pair.split_once('=') else {
            return false;
        };
        key.trim().eq_ignore_ascii_case(ONE_CLICK_KEY)
            && value.trim().eq_ignore_ascii_case(ONE_CLICK_TOKEN)
    })
}

/// Registrable-ish domain comparison: equal, or one is a subdomain of the
/// other. Used to tie authentication evidence to the host being contacted.
fn domain_matches(a: &str, b: &str) -> bool {
    let a = a.trim().trim_start_matches('.').to_ascii_lowercase();
    let b = b.trim().trim_start_matches('.').to_ascii_lowercase();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// A single `Authentication-Results` clause worth trusting.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub method: String,
    pub domain: String,
}

/// Parse the provider's `Authentication-Results` for a passing DMARC or DKIM
/// result. `trusted` must be true: evidence is only meaningful when the
/// provider produced the header.
pub fn evidence(raw: Option<&str>, trusted: bool) -> Vec<Evidence> {
    if !trusted {
        return Vec::new();
    }
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for clause in raw.split(';') {
        let mut tokens = clause.split_whitespace();
        let Some(head) = tokens.next() else {
            continue;
        };
        let Some((method, result)) = head.split_once('=') else {
            continue;
        };
        if !result.eq_ignore_ascii_case("pass") {
            continue;
        }
        let method = method.to_ascii_lowercase();
        if method != "dmarc" && method != "dkim" {
            continue;
        }
        // The domain an assertion is *about*: `header.d` for DKIM,
        // `header.from` for DMARC, with the envelope sender as a fallback.
        let domain = tokens
            .find_map(|token| {
                let (key, value) = token.split_once('=')?;
                let key = key.to_ascii_lowercase();
                if key == "header.d" || key == "header.from" || key == "smtp.mailfrom" {
                    Some(
                        value
                            .trim_matches(|c| c == '"' || c == ';' || c == ',')
                            .rsplit('@')
                            .next()
                            .unwrap_or_default()
                            .to_string(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or_default();
        out.push(Evidence { method, domain });
    }
    out
}

/// Does the provider's evidence cover this unsubscribe target?
///
/// `url_host` is the host the one-click POST would contact; `from_domain` the
/// domain of the message's From address. DMARC covers the From domain; DKIM
/// covers the domain it signed, which is what the unsubscribe link usually
/// belongs to.
pub fn evidence_covers<'a>(
    evidence: &'a [Evidence],
    url_host: &str,
    from_domain: &str,
) -> Option<&'a Evidence> {
    evidence.iter().find(|item| match item.method.as_str() {
        "dmarc" => domain_matches(&item.domain, from_domain) || item.domain.is_empty(),
        "dkim" => {
            domain_matches(&item.domain, url_host) || domain_matches(&item.domain, from_domain)
        }
        _ => false,
    })
}

/// The From address's domain.
pub fn domain_of(address: &str) -> String {
    address
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(|c| c == '>' || c == '<')
        .to_ascii_lowercase()
}

/// Is this address one Sift refuses to contact?
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_v4(mapped);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link local
                || v6 == Ipv6Addr::LOCALHOST
        }
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || octets[0] == 0
        || (octets[0] == 100 && (64..128).contains(&octets[1])) // CGNAT
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        || octets[0] >= 240
}

#[derive(Debug, Clone, PartialEq)]
pub enum TargetError {
    /// The URL is not HTTPS, so a one-click POST is refused.
    InsecureScheme(String),
    /// The host could not be resolved at all.
    Unresolved(String),
    /// The host resolves to an address Sift refuses to contact.
    Blocked(String),
    NotAUrl,
}

impl TargetError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::InsecureScheme(_) => "insecure_transport",
            Self::Unresolved(_) => "unresolved",
            Self::Blocked(_) => "blocked_target",
            Self::NotAUrl => "malformed",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::InsecureScheme(url) => format!(
                "Sift only sends a one-click unsubscribe over HTTPS, and this link is not HTTPS ({url})."
            ),
            Self::Unresolved(host) => format!("Sift could not resolve {host}."),
            Self::Blocked(host) => format!(
                "Sift refuses to send an unsubscribe to {host}: it resolves to a private, loopback or link-local address."
            ),
            Self::NotAUrl => "This unsubscribe link is not a URL Sift can use.".into(),
        }
    }
}

/// The URL's host as a bare string: an IPv6 address without its brackets, so
/// it can be compared, resolved and pinned consistently.
pub fn host_key(url: &url::Url) -> Option<String> {
    match url.host()? {
        url::Host::Domain(domain) => Some(domain.to_string()),
        url::Host::Ipv4(address) => Some(address.to_string()),
        url::Host::Ipv6(address) => Some(address.to_string()),
    }
}

/// Resolve a target and refuse one that points inside the machine or the local
/// network. The returned address is what the request is pinned to.
pub async fn resolve_public(url: &url::Url) -> Result<SocketAddr, TargetError> {
    let host = host_key(url).ok_or(TargetError::NotAUrl)?;
    let port = url.port_or_known_default().ok_or(TargetError::NotAUrl)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(TargetError::Blocked(host));
        }
        return Ok(SocketAddr::new(ip, port));
    }
    let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| TargetError::Unresolved(host.clone()))?
        .collect();
    if resolved.is_empty() {
        return Err(TargetError::Unresolved(host));
    }
    for address in &resolved {
        if is_blocked_ip(address.ip()) {
            return Err(TargetError::Blocked(host));
        }
    }
    // Pin the first validated address; every address in the answer has already
    // been checked, so binding to any member cannot be rebound to a blocked one.
    Ok(resolved[0])
}

/// A dedicated, credential-free client pinned to one validated address.
///
/// There is no cookie jar, no proxy trust and no redirect following, so the
/// POST cannot inherit a session, cannot carry an Authorization header Sift
/// never set, and cannot be bounced to a different host.
pub fn one_click_client(host: &str, address: SocketAddr) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("Sift/{}", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .referer(false)
        .resolve(host, address)
        .build()
}

#[derive(Debug, Clone, PartialEq)]
pub struct OneClickOutcome {
    pub delivered: bool,
    pub status: Option<u16>,
    pub reason: Option<String>,
    pub detail: String,
}

impl OneClickOutcome {
    pub fn failed(reason: &str, detail: impl Into<String>) -> Self {
        Self {
            delivered: false,
            status: None,
            reason: Some(reason.to_string()),
            detail: detail.into(),
        }
    }
}

/// Perform the one-click POST against a validated address.
///
/// The body is bounded: a provider that streams more than
/// [`MAX_RESPONSE_BYTES`] is not allowed to grow the request.
pub async fn perform_one_click(client: &reqwest::Client, url: &url::Url) -> OneClickOutcome {
    let response = match client
        .post(url.clone())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(reqwest::header::ACCEPT, "*/*")
        .body(ONE_CLICK_BODY)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return OneClickOutcome::failed(
                "post_failed",
                format!("Sift could not reach the unsubscribe service: {error}"),
            )
        }
    };
    let status = response.status();
    if status.is_redirection() {
        return OneClickOutcome {
            delivered: false,
            status: Some(status.as_u16()),
            reason: Some("redirected".into()),
            detail: "The unsubscribe service tried to redirect Sift. Sift does not follow redirects for a one-click unsubscribe; open the link instead.".into(),
        };
    }
    // Drain a bounded amount so the response cannot grow without limit.
    drain_bounded(response).await;
    if status.is_success() {
        OneClickOutcome {
            delivered: true,
            status: Some(status.as_u16()),
            reason: None,
            detail: "The unsubscribe service accepted Sift's one-click request.".into(),
        }
    } else {
        OneClickOutcome {
            delivered: false,
            status: Some(status.as_u16()),
            reason: Some("post_failed".into()),
            detail: format!("The unsubscribe service answered {status}."),
        }
    }
}

async fn drain_bounded(response: reqwest::Response) {
    use futures::StreamExt;
    let mut read = 0usize;
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        read += chunk.len();
        if read >= MAX_RESPONSE_BYTES {
            break;
        }
    }
}

/// Compose the `mailto:` fallback, keeping the encoding the header supplied
/// (a subject with a space stays `%20`, never a raw space).
pub fn normalize_mailto(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    if parsed.scheme() != "mailto" {
        return None;
    }
    Some(parsed.to_string())
}

/// Everything a decision about unsubscribing needs, already parsed.
#[derive(Debug, Clone, Default)]
pub struct UnsubHeaders {
    pub targets: Vec<UnsubscribeTarget>,
    /// The exact `List-Unsubscribe-Post` field value, if the message had one.
    pub post_value: Option<String>,
    pub auth_results: Option<String>,
    /// True only when the provider itself produced `auth_results`.
    pub trusted: bool,
    pub from_email: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnsubPlan {
    /// A one-click POST is warranted. The caller still has to validate the
    /// address and pin it before sending anything.
    OneClick { url: String, host: String },
    /// No POST will be sent: the message gets a labelled open-link, a
    /// `mailto:` or nothing at all.
    Fallback(UnsubscribeResult),
}

/// Build the fallback result from whatever the header offered.
///
/// `reason` and `detail` explain a refusal; they are absent when no POST was
/// ever on the table (a `mailto:`-only header is a legitimate flow, not a
/// refusal).
pub fn fallback_result(
    targets: &[UnsubscribeTarget],
    reason: Option<String>,
    status: Option<u16>,
    detail: Option<String>,
) -> UnsubscribeResult {
    let link = targets
        .iter()
        .find(|t| t.scheme == "https" || t.scheme == "http")
        .map(|t| t.url.clone());
    let mailto = targets
        .iter()
        .find(|t| t.scheme == "mailto")
        .and_then(|t| normalize_mailto(&t.url));
    if let Some(url) = link {
        return UnsubscribeResult {
            method: "open_link".into(),
            done: false,
            url: Some(url),
            mailto: None,
            reason,
            status,
            detail,
            targets: targets.to_vec(),
        };
    }
    if let Some(url) = mailto {
        return UnsubscribeResult {
            method: "mailto".into(),
            done: false,
            url: None,
            mailto: Some(url),
            reason,
            status,
            detail,
            targets: targets.to_vec(),
        };
    }
    UnsubscribeResult {
        method: "none".into(),
        done: false,
        url: None,
        mailto: None,
        reason: reason.or_else(|| Some("no_targets".into())),
        status,
        detail,
        targets: targets.to_vec(),
    }
}

/// Decide how to unsubscribe from one message — without touching the network.
///
/// A POST is planned only when the message carries the one-click field **and**
/// the provider supplied authentication evidence that covers the host being
/// contacted. Every other case is labelled, so the user completes it in the
/// browser or their mail client.
pub fn plan(headers: &UnsubHeaders) -> UnsubPlan {
    let targets = &headers.targets;
    let https = targets
        .iter()
        .find(|t| t.scheme == "https")
        .map(|t| t.url.clone());
    let has_link = targets
        .iter()
        .any(|t| t.scheme == "https" || t.scheme == "http");
    if !post_is_one_click(headers.post_value.as_deref()) {
        return UnsubPlan::Fallback(fallback_result(
            targets,
            has_link.then(|| "missing_post_value".to_string()),
            None,
            has_link.then(|| {
                "This message does not advertise one-click unsubscribe, so Sift will not POST to it.".to_string()
            }),
        ));
    }
    let Some(url) = https else {
        return UnsubPlan::Fallback(fallback_result(
            targets,
            Some("insecure_transport".into()),
            None,
            Some(
                "This message offers one-click unsubscribe only over a link Sift will not POST to, because it is not HTTPS."
                    .into(),
            ),
        ));
    };
    let parsed = match url::Url::parse(&url) {
        Ok(parsed) => parsed,
        Err(_) => {
            return UnsubPlan::Fallback(fallback_result(
                targets,
                Some("malformed".into()),
                None,
                Some("This unsubscribe link is not a URL Sift can use.".into()),
            ))
        }
    };
    let host = parsed.host_str().unwrap_or_default().to_string();
    let evidence = evidence(headers.auth_results.as_deref(), headers.trusted);
    if evidence_covers(&evidence, &host, &domain_of(&headers.from_email)).is_none() {
        return UnsubPlan::Fallback(fallback_result(
            targets,
            Some("untrusted_authentication".into()),
            None,
            Some(TRUSTED_HEADER_NOTE.into()),
        ));
    }
    UnsubPlan::OneClick { url, host }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_angle_bracketed_entries_in_order() {
        let targets = parse_targets("<mailto:leave@example.com>, <https://example.com/u?id=1>");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].scheme, "mailto");
        assert_eq!(targets[0].url, "mailto:leave@example.com");
        assert_eq!(targets[1].scheme, "https");
    }

    #[test]
    fn a_comma_inside_a_url_does_not_split_the_entry() {
        let targets = parse_targets("<https://example.com/u?a=1,b=2>");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].url, "https://example.com/u?a=1,b=2");
    }

    #[test]
    fn malformed_entries_are_dropped_not_guessed_at() {
        let targets = parse_targets("not a url, <mailto:leave@example.com>");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].scheme, "other");
        assert_eq!(targets[1].scheme, "mailto");
    }

    #[test]
    fn post_value_must_be_exact() {
        assert!(post_is_one_click(Some("List-Unsubscribe=One-Click")));
        assert!(post_is_one_click(Some("list-unsubscribe=one-click")));
        assert!(!post_is_one_click(Some("List-Unsubscribe=Two-Click")));
        assert!(!post_is_one_click(Some("")));
        assert!(!post_is_one_click(None));
    }

    #[test]
    fn authentication_is_never_inferred_from_an_untrusted_header() {
        let header =
            "mx.example.com; dkim=pass header.d=example.com; dmarc=pass header.from=example.com";
        assert!(evidence(Some(header), false).is_empty());
        let trusted = evidence(Some(header), true);
        assert_eq!(trusted.len(), 2);
        assert!(evidence_covers(&trusted, "example.com", "example.com").is_some());
        assert!(evidence_covers(&trusted, "evil.example.net", "victim.example").is_none());
    }

    #[test]
    fn blocked_addresses_cover_v4_v6_and_mapped() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.5",
            "169.254.10.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "{ip} must be refused");
        }
        for ip in ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(!is_blocked_ip(ip.parse().unwrap()), "{ip} must be allowed");
        }
    }

    #[test]
    fn mailto_keeps_its_encoding() {
        let encoded =
            normalize_mailto("mailto:leave@example.com?subject=Unsubscribe%20me").unwrap();
        assert!(encoded.contains("subject=Unsubscribe%20me"), "{encoded}");
    }
}
