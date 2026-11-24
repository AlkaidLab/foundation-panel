//! Why the panel's proxy answered a page request with the offline page, in one word, and the one log line that says so.
//!
//! The offline page is the answer to a failed request of the frame. A person who reports 「没有响应」 only ever sees that page; the log is the one place that says which
//! failure it was. Every answer with the page writes a line (once a minute per page and cause, with the number left out), and the line holds the cause, the host
//! address (scheme, host and port), the path of the page (no query), how long the request ran and whether the HTTP fallback was the one that failed. It holds no credential,
//! no query and no path of this computer.
//!
//! The module uses `std` only so its tests also run where the panel itself does not build.

use crate::local_credential::CredentialFailure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyFailureKind {
    /// Nothing listens at the host address.
    Refused,
    /// The connection to the host was not made within the connect timeout.
    ConnectTimeout,
    /// The connection was made and the host did not answer within the request's time.
    RequestTimeout,
    /// The host (or something in between) closed the connection under the request.
    Reset,
    /// The credential file could not be read; no request was made to the host.
    Credential(CredentialFailure),
    /// The host refused a connection a moment ago and the proxy answered without asking it again.
    FastFail,
    Other,
}

impl ProxyFailureKind {
    pub fn label(self) -> &'static str {
        match self {
            ProxyFailureKind::Refused => "refused",
            ProxyFailureKind::ConnectTimeout => "connect-timeout",
            ProxyFailureKind::RequestTimeout => "request-timeout",
            ProxyFailureKind::Reset => "reset",
            ProxyFailureKind::Credential(failure) => failure.label(),
            ProxyFailureKind::FastFail => "fast-fail",
            ProxyFailureKind::Other => "other",
        }
    }
}

/// What the proxy knows about a failed request: the whole chain of the error as text, what the HTTP client says of it, and the credential failure when the error is one.
pub struct FailureFacts<'a> {
    pub chain: &'a str,
    pub client_timeout: bool,
    pub client_connect: bool,
    pub credential: Option<CredentialFailure>,
}

pub fn classify(facts: &FailureFacts<'_>) -> ProxyFailureKind {
    if let Some(failure) = facts.credential {
        return ProxyFailureKind::Credential(failure);
    }
    let text = facts.chain.to_lowercase();
    if text.contains("refused") || text.contains("10061") {
        return ProxyFailureKind::Refused;
    }
    let timed_out = facts.client_timeout
        || text.contains("timed out")
        || text.contains("timeout")
        || text.contains("deadline has elapsed")
        || text.contains("10060")
        || text.contains("did not properly respond");
    if timed_out {
        let while_connecting = facts.client_connect
            || text.contains("(connect)")
            || text.contains("tcp connect")
            || text.contains("trying to connect")
            || text.contains("10060")
            || text.contains("did not properly respond");
        return if while_connecting { ProxyFailureKind::ConnectTimeout } else { ProxyFailureKind::RequestTimeout };
    }
    const RESET: &[&str] = &[
        "reset",
        "10054",
        "10053",
        "forcibly closed",
        "closed before message completed",
        "broken pipe",
        "connection aborted",
        "unexpected eof",
    ];
    if RESET.iter().any(|needle| text.contains(needle)) {
        return ProxyFailureKind::Reset;
    }
    ProxyFailureKind::Other
}

/// `scheme://host:port` of an address, nothing after it.
pub fn origin_of(url: &str) -> String {
    match url.find("://") {
        Some(at) => {
            let rest = &url[at + 3..];
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            // userinfo is never part of the host address that is written down
            let authority = &rest[..end];
            let authority = authority.rsplit('@').next().unwrap_or(authority);
            format!("{}://{}", &url[..at], authority)
        }
        None => String::new(),
    }
}

/// The path of an address, without the query and the fragment, short enough for a line.
pub fn path_of(url: &str) -> String {
    let rest = match url.find("://") {
        Some(at) => &url[at + 3..],
        None => url,
    };
    let from = rest.find('/').map(|at| &rest[at..]).unwrap_or("/");
    let end = from.find(['?', '#']).unwrap_or(from.len());
    let path = &from[..end];
    if path.chars().count() > 80 {
        let cut: String = path.chars().take(80).collect();
        format!("{cut}…")
    } else {
        path.to_string()
    }
}

pub struct OfflineLine<'a> {
    pub kind: ProxyFailureKind,
    /// The host address the request went to.
    pub target: &'a str,
    pub path: &'a str,
    /// How long the request ran before it failed; `None` when none was made.
    pub elapsed_ms: Option<u64>,
    /// The failure is the one of the HTTP request that followed a failed HTTPS request.
    pub http_fallback: bool,
    /// Seconds since the refusal the fast answer follows; only for `FastFail`.
    pub since_refusal_s: Option<u64>,
}

impl OfflineLine<'_> {
    pub fn text(&self) -> String {
        let mut line = format!("离线页 原因={} 目标={} 路径={}", self.kind.label(), self.target, self.path);
        if let Some(ms) = self.elapsed_ms {
            line.push_str(&format!(" 耗时={ms}ms"));
        }
        if self.http_fallback {
            line.push_str(" 降级HTTP=是");
        }
        if let Some(seconds) = self.since_refusal_s {
            line.push_str(&format!(" 距上次拒绝={seconds}s"));
        }
        line
    }

    /// The key a line is throttled by: one line a minute for the same page and cause.
    pub fn key(&self) -> String {
        format!("offline|{}|{}", self.path, self.kind.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(chain: &str) -> ProxyFailureKind {
        classify(&FailureFacts { chain, client_timeout: false, client_connect: false, credential: None })
    }

    #[test]
    fn a_refused_connection_is_refused_on_either_system() {
        assert_eq!(kind("error sending request for url (https://localhost:47990/): client error (Connect): tcp connect error: Connection refused (os error 111)"), ProxyFailureKind::Refused);
        assert_eq!(kind("error sending request: tcp connect error: No connection could be made because the target machine actively refused it. (os error 10061)"), ProxyFailureKind::Refused);
    }

    #[test]
    fn a_timeout_is_told_apart_by_where_it_happened() {
        assert_eq!(kind("error sending request for url (https://localhost:47990/): client error (Connect): tcp connect error: operation timed out"), ProxyFailureKind::ConnectTimeout);
        assert_eq!(kind("error sending request: tcp connect error: A connection attempt failed because the connected party did not properly respond after a period of time (os error 10060)"), ProxyFailureKind::ConnectTimeout);
        assert_eq!(kind("error sending request for url (https://localhost:47990/api/apps): operation timed out"), ProxyFailureKind::RequestTimeout);
        assert_eq!(kind("request or response body error: operation timed out"), ProxyFailureKind::RequestTimeout);
        assert_eq!(classify(&FailureFacts { chain: "error sending request", client_timeout: true, client_connect: true, credential: None }), ProxyFailureKind::ConnectTimeout);
        assert_eq!(classify(&FailureFacts { chain: "error sending request", client_timeout: true, client_connect: false, credential: None }), ProxyFailureKind::RequestTimeout);
    }

    #[test]
    fn a_closed_connection_is_a_reset() {
        for chain in [
            "error sending request: connection closed before message completed",
            "error sending request: An existing connection was forcibly closed by the remote host. (os error 10054)",
            "error reading a body from connection: Connection reset by peer (os error 104)",
            "error sending request: Broken pipe (os error 32)",
        ] {
            assert_eq!(kind(chain), ProxyFailureKind::Reset, "{chain}");
        }
    }

    #[test]
    fn the_credential_wins_over_the_words_of_the_text() {
        let facts = FailureFacts { chain: "Local host credential is unavailable (access denied) [os error 5]", client_timeout: false, client_connect: false, credential: Some(CredentialFailure::Denied) };
        assert_eq!(classify(&facts), ProxyFailureKind::Credential(CredentialFailure::Denied));
        assert_eq!(classify(&facts).label(), "credential-denied");
    }

    #[test]
    fn what_is_none_of_these_is_other() {
        assert_eq!(kind("invalid HTTP version parsed"), ProxyFailureKind::Other);
    }

    #[test]
    fn the_address_and_the_path_carry_nothing_else() {
        assert_eq!(origin_of("https://user:secret@localhost:47990/config?token=abc#x"), "https://localhost:47990");
        assert_eq!(origin_of("http://127.0.0.1:47989"), "http://127.0.0.1:47989");
        assert_eq!(origin_of("not an address"), "");
        assert_eq!(path_of("https://localhost:47990/config?token=abc#x"), "/config");
        assert_eq!(path_of("https://localhost:47990"), "/");
        assert!(path_of(&format!("http://h/{}", "a".repeat(200))).chars().count() <= 81);
    }

    #[test]
    fn the_line_names_the_cause_the_target_the_path_and_the_time_and_no_query() {
        let line = OfflineLine { kind: ProxyFailureKind::ConnectTimeout, target: "https://localhost:47990", path: "/config", elapsed_ms: Some(3004), http_fallback: false, since_refusal_s: None };
        assert_eq!(line.text(), "离线页 原因=connect-timeout 目标=https://localhost:47990 路径=/config 耗时=3004ms");
        let fallback = OfflineLine { kind: ProxyFailureKind::Reset, target: "http://localhost:47990", path: "/", elapsed_ms: Some(12), http_fallback: true, since_refusal_s: None };
        assert!(fallback.text().ends_with("耗时=12ms 降级HTTP=是"));
        let fast = OfflineLine { kind: ProxyFailureKind::FastFail, target: "https://localhost:47990", path: "/", elapsed_ms: None, http_fallback: false, since_refusal_s: Some(0) };
        assert_eq!(fast.text(), "离线页 原因=fast-fail 目标=https://localhost:47990 路径=/ 距上次拒绝=0s");
        assert_eq!(fast.key(), "offline|/|fast-fail");
    }
}
