//! GitHub REST API payloads (only the fields this connector uses).

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
}

/// Present on issues-listing entries that are pull requests.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestMarker {
    pub merged_at: Option<DateTime<Utc>>,
}

/// An entry of `GET /repos/{owner}/{repo}/issues` — GitHub lists issues and
/// pull requests together; `pull_request` marks the latter.
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub user: Option<Actor>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub pull_request: Option<PullRequestMarker>,
}

impl Issue {
    #[must_use]
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoRef {
    pub full_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CrossRefIssue {
    pub number: u64,
    pub repository: Option<RepoRef>,
}

/// The referencing side of a `cross-referenced` timeline entry.
#[derive(Debug, Clone, Deserialize)]
pub struct CrossRefSource {
    pub issue: Option<CrossRefIssue>,
}

/// One issue-timeline entry. The timeline is heterogeneous, so every kind's
/// fields are optional here; the mapper matches on `event` and skips kinds it
/// does not model (counting them honestly).
#[derive(Debug, Clone, Deserialize)]
pub struct TimelineEvent {
    pub event: String,
    pub id: Option<u64>,
    pub actor: Option<Actor>,
    /// `commented` entries carry the author as `user`, not `actor`.
    pub user: Option<Actor>,
    pub created_at: Option<DateTime<Utc>>,
    pub label: Option<Label>,
    pub assignee: Option<Actor>,
    pub source: Option<CrossRefSource>,
    /// `commented` entries carry the comment text.
    pub body: Option<String>,
}

/// An entry of `GET /repos/{owner}/{repo}/pulls/{n}/reviews`.
#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub id: u64,
    pub user: Option<Actor>,
    pub state: String,
    pub submitted_at: Option<DateTime<Utc>>,
}
