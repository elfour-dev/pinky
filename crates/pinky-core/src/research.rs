use std::{collections::HashSet, net::IpAddr, time::Duration};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::lookup_host;
use tokio_util::sync::CancellationToken;

pub const RESEARCH_SCHEMA_VERSION: u16 = 1;
pub const MAX_SEARCH_QUERIES: usize = 5;
pub const MAX_SEARCH_QUERY_BYTES: usize = 512;
pub const MAX_SEARCH_RESULTS: usize = 50;
pub const MAX_SEARCH_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_SEARCH_TEXT_BYTES: usize = 8 * 1024;
pub const DEFAULT_MAX_DOCUMENTS: u32 = 40;
pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 250 * 1024 * 1024;
pub const DEFAULT_MAX_DURATION_SECONDS: u64 = 20 * 60;
pub const DEFAULT_CRAWL_DEPTH: u8 = 2;
pub const DEFAULT_MAX_PAGES_PER_ORIGIN: u16 = 10;
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: u8 = 4;
pub const DEFAULT_MIN_ORIGIN_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum ResearchError {
    #[error("research schema version {0} is unsupported")]
    UnsupportedSchema(u16),
    #[error("research query must contain between 1 and {MAX_SEARCH_QUERY_BYTES} bytes")]
    InvalidQuery,
    #[error("research query list exceeds the {MAX_SEARCH_QUERIES}-query limit")]
    TooManyQueries,
    #[error("research budget field `{0}` must be greater than zero")]
    InvalidBudget(&'static str),
    #[error("research budget field `{0}` exceeds its safety limit")]
    ExcessiveBudget(&'static str),
    #[error("web URL is invalid")]
    InvalidUrl,
    #[error("web URL scheme must be http or https")]
    UnsupportedScheme,
    #[error("web URL must not contain credentials or a fragment")]
    UnsafeUrlMetadata,
    #[error("web URL host is blocked by policy")]
    BlockedDomain,
    #[error("web URL targets a loopback, private, link-local, or otherwise non-public address")]
    NonPublicAddress,
    #[error("web URL host could not be resolved to a public address")]
    UnresolvablePublicHost,
    #[error("SearXNG endpoint must be an HTTP loopback origin with an explicit port")]
    InvalidSearchEndpoint,
    #[error("SearXNG response was not valid JSON")]
    InvalidSearchResponse,
    #[error("SearXNG response exceeded the {limit}-byte limit")]
    SearchResponseTooLarge { limit: u64 },
    #[error("web response redirects are not followed automatically")]
    RedirectRejected,
    #[error("web request was cancelled")]
    Cancelled,
    #[error("web request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("web response returned HTTP status {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("web response exceeded the {limit}-byte limit")]
    ResponseTooLarge { limit: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchBudgetV1 {
    pub schema_version: u16,
    pub max_documents: u32,
    pub max_download_bytes: u64,
    pub max_duration_seconds: u64,
    pub crawl_depth: u8,
    pub max_pages_per_origin: u16,
    pub max_concurrent_requests: u8,
    pub min_origin_interval_ms: u64,
}

impl Default for ResearchBudgetV1 {
    fn default() -> Self {
        Self {
            schema_version: RESEARCH_SCHEMA_VERSION,
            max_documents: DEFAULT_MAX_DOCUMENTS,
            max_download_bytes: DEFAULT_MAX_DOWNLOAD_BYTES,
            max_duration_seconds: DEFAULT_MAX_DURATION_SECONDS,
            crawl_depth: DEFAULT_CRAWL_DEPTH,
            max_pages_per_origin: DEFAULT_MAX_PAGES_PER_ORIGIN,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            min_origin_interval_ms: DEFAULT_MIN_ORIGIN_INTERVAL.as_millis() as u64,
        }
    }
}

impl ResearchBudgetV1 {
    pub fn validate(&self) -> Result<(), ResearchError> {
        if self.schema_version != RESEARCH_SCHEMA_VERSION {
            return Err(ResearchError::UnsupportedSchema(self.schema_version));
        }
        for (name, value) in [
            ("max_documents", self.max_documents as u64),
            ("max_download_bytes", self.max_download_bytes),
            ("max_duration_seconds", self.max_duration_seconds),
            ("max_pages_per_origin", self.max_pages_per_origin as u64),
            (
                "max_concurrent_requests",
                self.max_concurrent_requests as u64,
            ),
            ("min_origin_interval_ms", self.min_origin_interval_ms),
        ] {
            if value == 0 {
                return Err(ResearchError::InvalidBudget(name));
            }
        }
        if self.crawl_depth > 2 {
            return Err(ResearchError::ExcessiveBudget("crawl_depth"));
        }
        if self.max_documents > DEFAULT_MAX_DOCUMENTS {
            return Err(ResearchError::ExcessiveBudget("max_documents"));
        }
        if self.max_download_bytes > DEFAULT_MAX_DOWNLOAD_BYTES {
            return Err(ResearchError::ExcessiveBudget("max_download_bytes"));
        }
        if self.max_duration_seconds > DEFAULT_MAX_DURATION_SECONDS {
            return Err(ResearchError::ExcessiveBudget("max_duration_seconds"));
        }
        if self.max_pages_per_origin > DEFAULT_MAX_PAGES_PER_ORIGIN {
            return Err(ResearchError::ExcessiveBudget("max_pages_per_origin"));
        }
        if self.max_concurrent_requests > DEFAULT_MAX_CONCURRENT_REQUESTS {
            return Err(ResearchError::ExcessiveBudget("max_concurrent_requests"));
        }
        Ok(())
    }
}

pub fn normalize_search_queries<I, S>(queries: I) -> Result<Vec<String>, ResearchError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for query in queries {
        let query = query.as_ref().trim();
        if query.is_empty() || query.len() > MAX_SEARCH_QUERY_BYTES {
            return Err(ResearchError::InvalidQuery);
        }
        if seen.insert(query.to_ascii_lowercase()) {
            normalized.push(query.to_owned());
        }
        if normalized.len() > MAX_SEARCH_QUERIES {
            return Err(ResearchError::TooManyQueries);
        }
    }
    if normalized.is_empty() {
        return Err(ResearchError::InvalidQuery);
    }
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl {
    url: Url,
    host: String,
}

impl PublicUrl {
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn host(&self) -> &str {
        &self.host
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPublicUrl {
    public_url: PublicUrl,
    address: std::net::SocketAddr,
}

impl ResolvedPublicUrl {
    pub fn public_url(&self) -> &PublicUrl {
        &self.public_url
    }

    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSearchResult {
    pub title: String,
    pub url: PublicUrl,
    pub snippet: String,
    pub engine: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Clone)]
pub struct SearxngClient {
    endpoint: Url,
    client: reqwest::Client,
}

impl SearxngClient {
    pub fn connect(endpoint: &str) -> Result<Self, ResearchError> {
        let endpoint = validate_search_endpoint(endpoint)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Pinky/1.0 public research")
            .build()
            .map_err(ResearchError::Request)?;
        Ok(Self { endpoint, client })
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    pub async fn search(
        &self,
        queries: &[String],
        policy: &PublicWebPolicy,
        cancellation: &CancellationToken,
    ) -> Result<Vec<PublicSearchResult>, ResearchError> {
        let queries = normalize_search_queries(queries.iter().map(String::as_str))?;
        let search_url = self
            .endpoint
            .join("search")
            .map_err(|_| ResearchError::InvalidSearchEndpoint)?;
        let mut results = Vec::new();
        let mut seen_urls = HashSet::new();

        for query in queries {
            let response = tokio::select! {
                _ = cancellation.cancelled() => return Err(ResearchError::Cancelled),
                response = self.client.get(search_url.clone())
                    .query(&[("q", query.as_str()), ("format", "json")])
                    .send() => response?,
            };
            if response.status().is_redirection() {
                return Err(ResearchError::RedirectRejected);
            }
            if !response.status().is_success() {
                return Err(ResearchError::HttpStatus(response.status()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_SEARCH_RESPONSE_BYTES)
            {
                return Err(ResearchError::SearchResponseTooLarge {
                    limit: MAX_SEARCH_RESPONSE_BYTES,
                });
            }
            let mut response = response;
            let mut body = Vec::new();
            while let Some(chunk) = tokio::select! {
                _ = cancellation.cancelled() => return Err(ResearchError::Cancelled),
                chunk = response.chunk() => chunk?,
            } {
                if body.len() as u64 + chunk.len() as u64 > MAX_SEARCH_RESPONSE_BYTES {
                    return Err(ResearchError::SearchResponseTooLarge {
                        limit: MAX_SEARCH_RESPONSE_BYTES,
                    });
                }
                body.extend_from_slice(&chunk);
            }
            let parsed: SearxngResponse =
                serde_json::from_slice(&body).map_err(|_| ResearchError::InvalidSearchResponse)?;
            for result in parsed.results {
                if results.len() == MAX_SEARCH_RESULTS {
                    break;
                }
                let Ok(url) = policy.validate_url(&result.url) else {
                    continue;
                };
                if !seen_urls.insert(url.url().as_str().to_owned()) {
                    continue;
                }
                results.push(PublicSearchResult {
                    title: truncate_text(&result.title, MAX_SEARCH_TEXT_BYTES),
                    url,
                    snippet: truncate_text(&result.content, MAX_SEARCH_TEXT_BYTES),
                    engine: result.engine,
                    published_at: result.published_at,
                });
            }
        }
        Ok(results)
    }
}

#[derive(Debug, Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

#[derive(Debug, Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    engine: Option<String>,
    #[serde(rename = "publishedDate", default)]
    published_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PublicWebPolicy {
    blocked_domains: HashSet<String>,
}

impl PublicWebPolicy {
    pub fn new<I, S>(blocked_domains: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            blocked_domains: blocked_domains
                .into_iter()
                .filter_map(|domain| normalize_domain(domain.as_ref()))
                .collect(),
        }
    }

    pub fn validate_url(&self, value: &str) -> Result<PublicUrl, ResearchError> {
        if value.chars().any(char::is_whitespace) {
            return Err(ResearchError::InvalidUrl);
        }
        let url = Url::parse(value).map_err(|_| ResearchError::InvalidUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ResearchError::UnsupportedScheme);
        }
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(ResearchError::UnsafeUrlMetadata);
        }
        let host = url
            .host_str()
            .and_then(normalize_domain)
            .ok_or(ResearchError::InvalidUrl)?;
        if is_blocked_domain(&host)
            || self
                .blocked_domains
                .iter()
                .any(|blocked| host == *blocked || host.ends_with(&format!(".{blocked}")))
        {
            return Err(ResearchError::BlockedDomain);
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if !is_public_ip(ip) {
                return Err(ResearchError::NonPublicAddress);
            }
        }
        Ok(PublicUrl { url, host })
    }

    /// Resolve every address before connecting. If a hostname has any private
    /// answer, fail closed instead of allowing DNS rebinding to cross the
    /// public/private boundary.
    pub async fn resolve_public_url(
        &self,
        value: &str,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedPublicUrl, ResearchError> {
        let public_url = self.validate_url(value)?;
        let port = public_url
            .url
            .port_or_known_default()
            .ok_or(ResearchError::InvalidUrl)?;
        let addresses = tokio::select! {
            _ = cancellation.cancelled() => return Err(ResearchError::Cancelled),
            addresses = lookup_host((public_url.host.as_str(), port)) => addresses
                .map_err(|_| ResearchError::UnresolvablePublicHost)?
                .collect::<Vec<_>>(),
        };
        if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
            return Err(ResearchError::NonPublicAddress);
        }
        let address = addresses
            .into_iter()
            .next()
            .ok_or(ResearchError::UnresolvablePublicHost)?;
        Ok(ResolvedPublicUrl {
            public_url,
            address,
        })
    }
}

pub fn build_public_client(resolved: &ResolvedPublicUrl) -> Result<reqwest::Client, ResearchError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Pinky/1.0 public research")
        .resolve(&resolved.public_url.host, resolved.address)
        .build()
        .map_err(ResearchError::Request)
}

fn validate_search_endpoint(value: &str) -> Result<Url, ResearchError> {
    if value.chars().any(char::is_whitespace) {
        return Err(ResearchError::InvalidSearchEndpoint);
    }
    let endpoint = Url::parse(value).map_err(|_| ResearchError::InvalidSearchEndpoint)?;
    if endpoint.scheme() != "http"
        || endpoint.host_str() != Some("127.0.0.1")
        || endpoint.port().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || !matches!(endpoint.path(), "" | "/")
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ResearchError::InvalidSearchEndpoint);
    }
    Ok(endpoint)
}

fn truncate_text(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub async fn fetch_public_body(
    resolved: &ResolvedPublicUrl,
    maximum_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<(reqwest::Response, Vec<u8>), ResearchError> {
    let client = build_public_client(resolved)?;
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(ResearchError::Cancelled),
        response = client.get(resolved.public_url.url.clone()).send() => response?,
    };
    if response.status().is_redirection() {
        return Err(ResearchError::RedirectRejected);
    }
    if !response.status().is_success() {
        return Err(ResearchError::HttpStatus(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(ResearchError::ResponseTooLarge {
            limit: maximum_bytes,
        });
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => return Err(ResearchError::Cancelled),
        chunk = response.chunk() => chunk?,
    } {
        if body.len() as u64 + chunk.len() as u64 > maximum_bytes {
            return Err(ResearchError::ResponseTooLarge {
                limit: maximum_bytes,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok((response, body))
}

fn normalize_domain(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    (!value.is_empty() && !value.contains('/')).then_some(value)
}

fn is_blocked_domain(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".lan")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_private()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_multicast()
                && !(ip.octets()[0] == 0)
                && !(ip.octets()[0] == 100 && (ip.octets()[1] & 0b1100_0000) == 0b0100_0000)
                && !(ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
                && !(ip.octets()[0] == 198 && (ip.octets()[1] == 18 || ip.octets()[1] == 19))
                && !(ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 2)
                && !(ip.octets()[0] == 198 && ip.octets()[1] == 51 && ip.octets()[2] == 100)
                && !(ip.octets()[0] == 203 && ip.octets()[1] == 0 && ip.octets()[2] == 113)
        }
        IpAddr::V6(ip) => {
            !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && !ip.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_is_bounded_and_valid() {
        let budget = ResearchBudgetV1::default();
        budget.validate().unwrap();
        assert_eq!(budget.max_documents, 40);
        assert_eq!(budget.max_download_bytes, 250 * 1024 * 1024);
    }

    #[test]
    fn rejects_budget_expansion_beyond_default_limits() {
        let mut budget = ResearchBudgetV1::default();
        budget.max_documents += 1;
        assert!(matches!(
            budget.validate(),
            Err(ResearchError::ExcessiveBudget("max_documents"))
        ));
    }

    #[test]
    fn normalizes_and_caps_search_queries() {
        let queries = normalize_search_queries([" Jonah ", "jonah", "Project Alder"]).unwrap();
        assert_eq!(queries, vec!["Jonah", "Project Alder"]);
        let too_many = (0..=MAX_SEARCH_QUERIES)
            .map(|index| format!("query {index}"))
            .collect::<Vec<_>>();
        assert!(matches!(
            normalize_search_queries(too_many),
            Err(ResearchError::TooManyQueries)
        ));
    }

    #[test]
    fn accepts_public_urls_and_applies_user_domain_blocks() {
        let policy = PublicWebPolicy::new(["blocked.example"]);
        let url = policy
            .validate_url("https://example.com/articles?id=42")
            .unwrap();
        assert_eq!(url.host(), "example.com");
        assert!(matches!(
            policy.validate_url("https://sub.blocked.example/page"),
            Err(ResearchError::BlockedDomain)
        ));
    }

    #[test]
    fn rejects_private_metadata_and_unsafe_targets() {
        let policy = PublicWebPolicy::default();
        for value in [
            "http://127.0.0.1:8080",
            "http://192.168.1.10/router",
            "http://localhost/dashboard",
            "http://printer.local/status",
            "https://user:password@example.com/",
            "file:///etc/passwd",
            "https://example.com/page#private-fragment",
        ] {
            assert!(policy.validate_url(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn rejects_non_public_ip_ranges() {
        for value in [
            "https://10.0.0.1/",
            "https://100.64.0.1/",
            "https://192.0.0.1/",
            "https://198.18.0.1/",
            "https://203.0.113.7/",
            "https://[::1]/",
            "https://[fd00::1]/",
        ] {
            assert!(
                matches!(
                    PublicWebPolicy::default().validate_url(value),
                    Err(ResearchError::NonPublicAddress)
                ),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn only_accepts_a_loopback_searxng_endpoint() {
        let client = SearxngClient::connect("http://127.0.0.1:8080").unwrap();
        assert_eq!(client.endpoint().as_str(), "http://127.0.0.1:8080/");
        for endpoint in [
            "https://127.0.0.1:8080",
            "http://localhost:8080",
            "http://127.0.0.2:8080",
            "http://127.0.0.1",
            "http://127.0.0.1:8080/search",
            "http://user@127.0.0.1:8080",
        ] {
            assert!(
                matches!(
                    SearxngClient::connect(endpoint),
                    Err(ResearchError::InvalidSearchEndpoint)
                ),
                "accepted {endpoint}"
            );
        }
    }

    #[tokio::test]
    async fn cancellation_wins_before_dns_resolution() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            PublicWebPolicy::default()
                .resolve_public_url("https://example.com/", &cancellation)
                .await,
            Err(ResearchError::Cancelled)
        ));
    }
}
