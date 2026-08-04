//! Outbound network allowlist / denylist for HTTP tools.
//!
//! Applied to tools that open arbitrary or fixed remote URLs (`webfetch`,
//! `websearch`, GitHub REST helpers). Shell network remains binary
//! (`security.sandbox_network`); domain filtering for shell would need a
//! userspace proxy and is out of scope here.
//!
//! ## Semantics
//!
//! - Empty allowlist → all hosts allowed (subject to denylist).
//! - Non-empty allowlist → host must match at least one pattern.
//! - Denylist always wins over allowlist.
//!
//! ## Pattern syntax
//!
//! - `example.com` — apex and any subdomain (`api.example.com`).
//! - `*.example.com` — subdomains only (not the apex).
//! - `*` — match any host.
//!
//! Matching is case-insensitive. Ports and userinfo are stripped before match.

use serde::{Deserialize, Serialize};

/// Resolved network policy carried on [`crate::ToolContext`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetworkPolicy {
    /// Allowed host patterns. Empty means unrestricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowlist: Vec<String>,
    /// Always-blocked host patterns (wins over allowlist).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denylist: Vec<String>,
}

impl NetworkPolicy {
    pub fn unrestricted() -> Self {
        Self::default()
    }

    pub fn is_restricted(&self) -> bool {
        !self.allowlist.is_empty() || !self.denylist.is_empty()
    }

    /// Whether `host` may be contacted under this policy.
    pub fn is_host_allowed(&self, host: &str) -> bool {
        let host = normalize_host(host);
        if host.is_empty() {
            return false;
        }
        if self
            .denylist
            .iter()
            .any(|p| host_matches_pattern(&host, p))
        {
            return false;
        }
        if self.allowlist.is_empty() {
            return true;
        }
        self.allowlist
            .iter()
            .any(|p| host_matches_pattern(&host, p))
    }

    /// Parse host from `url` and check policy. On block, returns an error
    /// message suitable for a tool result.
    pub fn ensure_url_allowed(&self, url: &str) -> Result<(), String> {
        if !self.is_restricted() {
            // Still validate scheme when unrestricted? No — leave that to the
            // HTTP client. Only enforce host policy when lists are set.
            // Actually denylist empty + allowlist empty → skip parse.
            return Ok(());
        }
        let host = host_from_url(url)?;
        if self.is_host_allowed(&host) {
            Ok(())
        } else {
            Err(blocked_message(&host, url, self))
        }
    }

    /// Like [`ensure_url_allowed`] but always parses the host when restricted
    /// *or* when we want a consistent check path for tests/tools.
    pub fn check_url(&self, url: &str) -> Result<(), String> {
        let host = host_from_url(url)?;
        if self.is_host_allowed(&host) {
            Ok(())
        } else {
            Err(blocked_message(&host, url, self))
        }
    }
}

fn blocked_message(host: &str, url: &str, policy: &NetworkPolicy) -> String {
    let mut msg = format!("Network policy blocked host `{host}` for URL: {url}");
    if !policy.allowlist.is_empty() {
        msg.push_str(&format!(
            "\nAllowed patterns: {}",
            policy.allowlist.join(", ")
        ));
    }
    if !policy.denylist.is_empty() {
        msg.push_str(&format!(
            "\nDenied patterns: {}",
            policy.denylist.join(", ")
        ));
    }
    msg.push_str(
        "\nConfigure `security.network_allowlist` / `security.network_denylist` in config.toml \
         (or WHYCODE_NETWORK_ALLOWLIST / WHYCODE_NETWORK_DENYLIST).",
    );
    msg
}

/// Lowercase host, strip trailing dots and IPv6 brackets.
pub fn normalize_host(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.starts_with('[') && h.ends_with(']') && h.len() >= 2 {
        h[1..h.len() - 1].to_string()
    } else {
        h
    }
}

/// Extract hostname from an `http://` or `https://` URL.
pub fn host_from_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("URL is empty".to_string());
    }

    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if let Some(r) = url.strip_prefix("http://") {
        r
    } else if let Some(scheme_end) = url.find("://") {
        let scheme = &url[..scheme_end];
        return Err(format!(
            "unsupported URL scheme `{scheme}` (only http and https are allowed for network tools)"
        ));
    } else {
        return Err(format!(
            "URL must start with http:// or https:// (got: {url})"
        ));
    };

    if rest.is_empty() {
        return Err(format!("could not parse host from URL: {url}"));
    }

    // authority ends at first `/`, `?`, or `#`
    let authority_end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(format!("could not parse host from URL: {url}"));
    }

    // userinfo@host:port — take after last @
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };

    let host = if hostport.starts_with('[') {
        // [ipv6] or [ipv6]:port
        match hostport.find(']') {
            Some(end) => &hostport[1..end],
            None => {
                return Err(format!("invalid IPv6 host in URL: {url}"));
            }
        }
    } else {
        // host or host:port (first colon separates port for IPv4/hostname)
        hostport.split(':').next().unwrap_or(hostport)
    };

    let host = normalize_host(host);
    if host.is_empty() {
        return Err(format!("could not parse host from URL: {url}"));
    }
    Ok(host)
}

/// Whether `host` matches a single pattern (see module docs).
pub fn host_matches_pattern(host: &str, pattern: &str) -> bool {
    let host = normalize_host(host);
    let pattern = normalize_host(pattern);
    if host.is_empty() || pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        if suffix.is_empty() {
            return false;
        }
        // Subdomains only: `a.suffix` matches; `suffix` does not.
        host.len() > suffix.len()
            && host.ends_with(suffix)
            && host.as_bytes().get(host.len() - suffix.len() - 1) == Some(&b'.')
    } else {
        host == pattern
            || (host.len() > pattern.len()
                && host.ends_with(&pattern)
                && host.as_bytes().get(host.len() - pattern.len() - 1) == Some(&b'.'))
    }
}

/// Split a comma- or whitespace-separated env list into patterns.
pub fn parse_domain_list(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_from_https_with_path_and_port() {
        assert_eq!(
            host_from_url("https://api.github.com:443/repos/x").unwrap(),
            "api.github.com"
        );
    }

    #[test]
    fn host_from_userinfo() {
        assert_eq!(
            host_from_url("https://user:pass@example.com/a").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn host_from_ipv6() {
        assert_eq!(
            host_from_url("http://[::1]:8080/x").unwrap(),
            "::1"
        );
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(host_from_url("ftp://example.com").is_err());
        assert!(host_from_url("example.com/path").is_err());
    }

    #[test]
    fn apex_pattern_matches_subdomains() {
        assert!(host_matches_pattern("example.com", "example.com"));
        assert!(host_matches_pattern("api.example.com", "example.com"));
        assert!(host_matches_pattern("a.b.example.com", "example.com"));
        assert!(!host_matches_pattern("notexample.com", "example.com"));
        assert!(!host_matches_pattern("example.com.evil", "example.com"));
    }

    #[test]
    fn star_prefix_subdomains_only() {
        assert!(host_matches_pattern("api.example.com", "*.example.com"));
        assert!(!host_matches_pattern("example.com", "*.example.com"));
        assert!(host_matches_pattern("a.b.example.com", "*.example.com"));
    }

    #[test]
    fn star_matches_all() {
        assert!(host_matches_pattern("anything.test", "*"));
    }

    #[test]
    fn unrestricted_allows_all() {
        let p = NetworkPolicy::unrestricted();
        assert!(p.is_host_allowed("evil.example"));
        assert!(p.ensure_url_allowed("https://evil.example/x").is_ok());
    }

    #[test]
    fn allowlist_blocks_unknown() {
        let p = NetworkPolicy {
            allowlist: vec!["github.com".into(), "crates.io".into()],
            denylist: vec![],
        };
        assert!(p.is_host_allowed("api.github.com"));
        assert!(p.is_host_allowed("crates.io"));
        assert!(!p.is_host_allowed("evil.com"));
        assert!(p.check_url("https://api.github.com/x").is_ok());
        assert!(p.check_url("https://evil.com/x").is_err());
    }

    #[test]
    fn denylist_wins() {
        let p = NetworkPolicy {
            allowlist: vec!["example.com".into()],
            denylist: vec!["tracking.example.com".into()],
        };
        assert!(p.is_host_allowed("example.com"));
        assert!(p.is_host_allowed("docs.example.com"));
        assert!(!p.is_host_allowed("tracking.example.com"));
    }

    #[test]
    fn denylist_alone() {
        let p = NetworkPolicy {
            allowlist: vec![],
            denylist: vec!["bad.com".into()],
        };
        assert!(p.is_host_allowed("good.com"));
        assert!(!p.is_host_allowed("bad.com"));
        assert!(!p.is_host_allowed("sub.bad.com"));
    }

    #[test]
    fn parse_domain_list_splits() {
        assert_eq!(
            parse_domain_list("github.com, crates.io *.npmjs.org"),
            vec!["github.com", "crates.io", "*.npmjs.org"]
        );
    }
}
