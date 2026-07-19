//! Core-owned SSRF-safe bounded fetcher shared by signed package and Source
//! ingestion paths. Bundle runtimes never receive this client or network
//! capability.

use futures::StreamExt as _;
use reqwest::{header, Url};
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct SafeFetchPolicy<'a> {
    pub max_bytes: usize,
    pub max_redirects: usize,
    pub allowed_content_types: &'static [&'static str],
    pub allowed_domains: Option<&'a [String]>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct SafeFetchResponse {
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub redirects: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SafeFetchError {
    #[error("invalid source URL: {0}")]
    InvalidUrl(String),
    #[error("source URL must use https:443 without credentials or fragments")]
    UrlPolicy,
    #[error("source host is outside the signed collection domain allowlist")]
    DomainBlocked,
    #[error("source host cannot be resolved: {0}")]
    Dns(String),
    #[error("source host must resolve only to public unicast addresses")]
    PrivateAddress,
    #[error("source redirect is missing a valid Location")]
    InvalidRedirect,
    #[error("source redirect limit exceeded")]
    RedirectLimit,
    #[error("source returned HTTP {0}")]
    HttpStatus(u16),
    #[error("source content type {0:?} is not allowed")]
    ContentType(String),
    #[error("source body exceeds the {0}-byte limit")]
    TooLarge(usize),
    #[error("source returned an empty body")]
    EmptyBody,
    #[error("source transport failed: {0}")]
    Transport(String),
}

impl SafeFetchError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidUrl(_) | Self::UrlPolicy => "fetch_url_invalid",
            Self::DomainBlocked => "fetch_domain_blocked",
            Self::Dns(_) => "fetch_dns_failed",
            Self::PrivateAddress => "fetch_ssrf_blocked",
            Self::InvalidRedirect | Self::RedirectLimit => "fetch_redirect_blocked",
            Self::HttpStatus(_) => "fetch_http_status",
            Self::ContentType(_) => "fetch_content_type_blocked",
            Self::TooLarge(_) => "fetch_too_large",
            Self::EmptyBody => "fetch_empty_body",
            Self::Transport(_) => "fetch_transport_failed",
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::HttpStatus(status) => Some(*status),
            _ => None,
        }
    }
}

pub async fn fetch_public_https(
    raw_url: &str,
    policy: SafeFetchPolicy<'_>,
) -> Result<SafeFetchResponse, SafeFetchError> {
    if policy.timeout.is_zero() {
        return Err(SafeFetchError::Transport(
            "source deadline is exhausted".into(),
        ));
    }
    tokio::time::timeout(policy.timeout, fetch_public_https_inner(raw_url, policy))
        .await
        .map_err(|_| SafeFetchError::Transport("source deadline exceeded".into()))?
}

async fn fetch_public_https_inner(
    raw_url: &str,
    policy: SafeFetchPolicy<'_>,
) -> Result<SafeFetchResponse, SafeFetchError> {
    let mut url =
        Url::parse(raw_url).map_err(|error| SafeFetchError::InvalidUrl(error.to_string()))?;
    let mut redirects = 0usize;
    loop {
        validate_url(&url, policy.allowed_domains)?;
        let host = url
            .host_str()
            .ok_or_else(|| SafeFetchError::InvalidUrl("host is missing".into()))?;
        let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, 443))
            .await
            .map_err(|error| SafeFetchError::Dns(error.to_string()))?
            .collect();
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|address| !is_public_unicast(address.ip()))
        {
            return Err(SafeFetchError::PrivateAddress);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(policy.timeout.min(Duration::from_secs(20)))
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|error| SafeFetchError::Transport(error.to_string()))?;
        let response = client
            .get(url.clone())
            .header(header::ACCEPT, policy.allowed_content_types.join(", "))
            .header(header::USER_AGENT, "Gadgetron-Knowledge-Collector/0.7")
            .send()
            .await
            .map_err(|error| SafeFetchError::Transport(error.to_string()))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(SafeFetchError::InvalidRedirect)?;
            url = checked_redirect(&url, location, redirects, policy.max_redirects)?;
            redirects += 1;
            continue;
        }
        let status = checked_http_status(response.status())?;
        let content_type = normalized_content_type(response.headers());
        if !policy
            .allowed_content_types
            .iter()
            .any(|allowed| *allowed == content_type)
        {
            return Err(SafeFetchError::ContentType(content_type));
        }
        checked_content_length(response.content_length(), policy.max_bytes)?;
        let final_url = url.to_string();
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| SafeFetchError::Transport(error.to_string()))?;
            checked_stream_size(bytes.len(), chunk.len(), policy.max_bytes)?;
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(SafeFetchError::EmptyBody);
        }
        return Ok(SafeFetchResponse {
            final_url,
            status,
            content_type,
            bytes,
            redirects,
        });
    }
}

/// Apply the redirect-count and relative-URL rules used by the Core fetch loop.
///
/// This small policy seam keeps redirect-limit behavior deterministic in
/// contract tests without weakening DNS pinning or allowing a test transport
/// in production.
pub fn checked_redirect(
    current: &Url,
    location: &str,
    redirects: usize,
    max_redirects: usize,
) -> Result<Url, SafeFetchError> {
    if redirects >= max_redirects {
        return Err(SafeFetchError::RedirectLimit);
    }
    current
        .join(location)
        .map_err(|_| SafeFetchError::InvalidRedirect)
}

/// Reject a non-success response before reading or retaining its body.
pub fn checked_http_status(status: reqwest::StatusCode) -> Result<u16, SafeFetchError> {
    if !status.is_success() {
        return Err(SafeFetchError::HttpStatus(status.as_u16()));
    }
    Ok(status.as_u16())
}

/// Reject an advertised response body that exceeds the Core fetch cap.
pub fn checked_content_length(
    content_length: Option<u64>,
    max_bytes: usize,
) -> Result<(), SafeFetchError> {
    if content_length.is_some_and(|length| length > max_bytes as u64) {
        return Err(SafeFetchError::TooLarge(max_bytes));
    }
    Ok(())
}

/// Reject a streamed response before appending a chunk past the Core fetch cap.
pub fn checked_stream_size(
    accumulated: usize,
    chunk: usize,
    max_bytes: usize,
) -> Result<(), SafeFetchError> {
    if accumulated.saturating_add(chunk) > max_bytes {
        return Err(SafeFetchError::TooLarge(max_bytes));
    }
    Ok(())
}

fn validate_url(url: &Url, allowed_domains: Option<&[String]>) -> Result<(), SafeFetchError> {
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(SafeFetchError::UrlPolicy);
    }
    if allowed_domains.is_some_and(|domains| {
        url.host_str().is_none_or(|host| {
            !domains
                .iter()
                .any(|domain| host.eq_ignore_ascii_case(domain))
        })
    }) {
        return Err(SafeFetchError::DomainBlocked);
    }
    Ok(())
}

fn normalized_content_type(headers: &header::HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn is_public_unicast(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [first, second, third, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_multicast()
                && !ip.is_unspecified()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && first != 0
                && first < 224
                && !(first == 100 && (64..=127).contains(&second))
                && !(first == 192 && second == 0 && third == 0)
                && !(first == 192 && second == 88 && third == 99)
                && !(first == 198 && matches!(second, 18 | 19))
        }
        std::net::IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_public_unicast(std::net::IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            segments[0] & 0xe000 == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_fetch_blocks_private_special_and_mapped_addresses() {
        assert!(is_public_unicast("8.8.8.8".parse().unwrap()));
        assert!(is_public_unicast("2606:4700:4700::1111".parse().unwrap()));
        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "192.0.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!is_public_unicast(blocked.parse().unwrap()), "{blocked}");
        }
    }

    #[tokio::test]
    async fn source_fetch_rejects_loopback_before_transport() {
        let error = fetch_public_https(
            "https://127.0.0.1/metadata",
            SafeFetchPolicy {
                max_bytes: 1024,
                max_redirects: 3,
                allowed_content_types: &["text/html"],
                allowed_domains: None,
                timeout: Duration::from_secs(1),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "fetch_ssrf_blocked");
    }

    #[tokio::test]
    async fn source_fetch_applies_signed_domain_before_transport() {
        let allowed = vec!["guide.michelin.com".to_string()];
        let error = fetch_public_https(
            "https://example.com/article",
            SafeFetchPolicy {
                max_bytes: 1024,
                max_redirects: 3,
                allowed_content_types: &["text/html"],
                allowed_domains: Some(&allowed),
                timeout: Duration::from_secs(1),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "fetch_domain_blocked");
    }
}
