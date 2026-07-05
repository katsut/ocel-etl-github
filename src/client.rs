//! GitHub REST API client: pagination and rate-limit backoff.
//!
//! The HTTP layer is abstracted behind [`HttpGet`] so pagination and backoff
//! are fully testable without a network; [`GithubClient::from_env`] wires in
//! the real `reqwest`-based transport. `GITHUB_TOKEN` is optional: public
//! repositories work anonymously (60 requests/hour), a token raises the limit
//! and unlocks private repositories.

use std::thread::sleep;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::models::{Issue, Review, TimelineEvent};

const BASE_URL: &str = "https://api.github.com";
/// Page size for list endpoints (the GitHub API maximum).
const PAGE: usize = 100;
/// Give up after this many consecutive rate-limit retries per request.
const MAX_RETRIES: u32 = 5;
/// Longest single backoff sleep. `X-RateLimit-Reset` can point up to an hour
/// ahead; retrying sooner at worst burns one request.
const MAX_BACKOFF_SECS: u64 = 120;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http transport error: {0}")]
    Transport(String),

    #[error("API returned status {status} for {context}")]
    Status { status: u16, context: String },

    #[error("rate limited; gave up after {0} retries")]
    RateLimited(u32),

    #[error("failed to parse response for {context}: {message}")]
    Parse { context: String, message: String },
}

/// A minimal HTTP response for [`HttpGet`] implementations.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// Seconds to wait before retrying (from rate-limit headers), if present.
    pub retry_after: Option<u64>,
    pub body: String,
}

/// The transport abstraction: perform a GET against a fully-formed URL.
pub trait HttpGet {
    fn get(&self, url: &str) -> Result<HttpResponse, ClientError>;
}

/// `reqwest`-based transport (blocking, rustls). Sends the GitHub media type,
/// API version, a User-Agent (required by GitHub), and the token if present.
#[derive(Debug)]
pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
    token: Option<String>,
}

impl ReqwestHttp {
    #[must_use]
    pub fn new(token: Option<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            token,
        }
    }
}

impl HttpGet for ReqwestHttp {
    fn get(&self, url: &str) -> Result<HttpResponse, ClientError> {
        let mut request = self
            .client
            .get(url)
            .header("User-Agent", "ocel-etl-github")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request
            .send()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .or_else(|| response.headers().get("X-RateLimit-Reset"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| {
                // X-RateLimit-Reset is an epoch timestamp; Retry-After is seconds.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                if v > now {
                    v - now
                } else {
                    v.min(60)
                }
            });
        let body = response
            .text()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(HttpResponse {
            status,
            retry_after,
            body,
        })
    }
}

/// GitHub API client over an [`HttpGet`] transport.
#[derive(Debug)]
pub struct GithubClient<H> {
    http: H,
    base_url: String,
}

impl GithubClient<ReqwestHttp> {
    /// Build a client from the environment; `GITHUB_TOKEN` is optional.
    #[must_use]
    pub fn from_env() -> Self {
        let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
        Self::new(ReqwestHttp::new(token), BASE_URL)
    }
}

impl<H: HttpGet> GithubClient<H> {
    /// Create a client over an arbitrary transport (tests inject fakes here).
    pub fn new(http: H, base_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// Issues **and** pull requests of a repository (GitHub lists them
    /// together), oldest-updated first, optionally only those updated at or
    /// after `since`.
    pub fn issues(
        &self,
        repo: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Issue>, ClientError> {
        let since_param = since.map(|s| s.to_rfc3339_opts(SecondsFormat::Secs, true));
        self.paginated(&format!("/repos/{repo}/issues"), "issues", |params| {
            params.push(("state", "all".to_owned()));
            params.push(("sort", "updated".to_owned()));
            params.push(("direction", "asc".to_owned()));
            if let Some(s) = &since_param {
                params.push(("since", s.clone()));
            }
        })
    }

    /// The timeline of one issue / pull request.
    pub fn timeline(&self, repo: &str, number: u64) -> Result<Vec<TimelineEvent>, ClientError> {
        self.paginated(
            &format!("/repos/{repo}/issues/{number}/timeline"),
            "timeline",
            |_| {},
        )
    }

    /// The reviews of one pull request.
    pub fn reviews(&self, repo: &str, number: u64) -> Result<Vec<Review>, ClientError> {
        self.paginated(
            &format!("/repos/{repo}/pulls/{number}/reviews"),
            "reviews",
            |_| {},
        )
    }

    fn paginated<T: DeserializeOwned>(
        &self,
        path: &str,
        context: &str,
        extra: impl Fn(&mut Vec<(&'static str, String)>),
    ) -> Result<Vec<T>, ClientError> {
        let mut items: Vec<T> = Vec::new();
        let mut page = 1usize;
        loop {
            let mut params: Vec<(&'static str, String)> =
                vec![("per_page", PAGE.to_string()), ("page", page.to_string())];
            extra(&mut params);
            let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let url = format!("{}{}?{}", self.base_url, path, query.join("&"));
            let batch: Vec<T> = self.get_json(&url, context)?;
            let full = batch.len() == PAGE;
            items.extend(batch);
            if !full {
                return Ok(items);
            }
            page += 1;
        }
    }

    /// GET with backoff, parsing the JSON body. GitHub signals rate limits as
    /// 429 or as 403 with reset headers; transient transport failures (dropped
    /// connections) are retried too, since a multi-thousand-request pull only
    /// writes its output at the very end. Waits are capped at
    /// [`MAX_BACKOFF_SECS`] — `X-RateLimit-Reset` can be up to an hour away,
    /// and one uncapped sleep once stalled a pull for 80 minutes. Every
    /// backoff is logged to stderr so a throttled pull is distinguishable
    /// from a hung one.
    fn get_json<T: DeserializeOwned>(&self, url: &str, context: &str) -> Result<T, ClientError> {
        let mut retries = 0;
        loop {
            let response = match self.http.get(url) {
                Ok(response) => response,
                Err(err @ ClientError::Transport(_)) => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return Err(err);
                    }
                    backoff(context, retries, u64::from(retries), &err.to_string());
                    continue;
                }
                Err(err) => return Err(err),
            };
            match response.status {
                200 => {
                    return serde_json::from_str(&response.body).map_err(|e| ClientError::Parse {
                        context: context.to_owned(),
                        message: e.to_string(),
                    });
                }
                status @ 429 => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return Err(ClientError::RateLimited(MAX_RETRIES));
                    }
                    let wait = response.retry_after.unwrap_or(1).min(MAX_BACKOFF_SECS);
                    backoff(context, retries, wait, &format!("status {status}"));
                }
                status @ 403 if response.retry_after.is_some() => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return Err(ClientError::RateLimited(MAX_RETRIES));
                    }
                    let wait = response.retry_after.unwrap_or(1).min(MAX_BACKOFF_SECS);
                    backoff(context, retries, wait, &format!("status {status}"));
                }
                status => {
                    return Err(ClientError::Status {
                        status,
                        context: context.to_owned(),
                    });
                }
            }
        }
    }
}

fn backoff(context: &str, attempt: u32, wait_secs: u64, reason: &str) {
    eprintln!("  {context}: {reason}; retry {attempt}/{MAX_RETRIES} in {wait_secs}s");
    sleep(Duration::from_secs(wait_secs));
}
