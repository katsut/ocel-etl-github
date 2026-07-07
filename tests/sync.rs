//! Incremental sync of the `closes` O2O link: prune + remap must equal a
//! full re-pull — the link is restored when the PR refreshes, and not
//! duplicated when the closed issue refreshes.

use std::collections::BTreeSet;
use std::convert::Infallible;

use chrono::{DateTime, TimeZone, Utc};
use ocel::Ocel;
use ocel_etl::StagingLog;
use ocel_etl_github::mapper::RepoMapper;
use ocel_etl_github::models::{
    Actor, CrossRefIssue, CrossRefSource, Issue, PullRequestMarker, RepoRef, TimelineEvent,
};
use ocel_etl_github::sync::{prune_refreshed, repair_closes_links};

fn ts(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 9, minute, 0).unwrap()
}

fn actor(login: &str) -> Actor {
    Actor {
        login: login.into(),
    }
}

fn issue(number: u64, pr: bool) -> Issue {
    Issue {
        number,
        title: format!("thing {number}"),
        state: "open".into(),
        user: Some(actor("alice")),
        created_at: ts(0),
        updated_at: ts(30),
        pull_request: pr.then_some(PullRequestMarker { merged_at: None }),
    }
}

fn entry(event: &str, minute: u32) -> TimelineEvent {
    TimelineEvent {
        event: event.into(),
        id: Some(u64::from(minute)),
        actor: Some(actor("bob")),
        user: Some(actor("bob")),
        created_at: Some(ts(minute)),
        label: None,
        assignee: None,
        source: None,
        body: None,
        commit_id: None,
    }
}

fn closed_by(sha: &str, minute: u32) -> TimelineEvent {
    let mut closed = entry("closed", minute);
    closed.commit_id = Some(sha.into());
    closed
}

/// The scripted stand-in for `GET /commits/{sha}/pulls`: PR #2 merged it.
#[allow(clippy::unnecessary_wraps)] // scripted resolutions are Results by design
fn resolve_abc(sha: &str) -> Result<Vec<u64>, Infallible> {
    assert_eq!(sha, "abc123");
    Ok(vec![2])
}

fn no_resolution(_sha: &str) -> Result<Vec<u64>, Infallible> {
    panic!("resolve must not be called")
}

fn new_mapper() -> RepoMapper<'static> {
    RepoMapper::new("o/r", BTreeSet::from([1, 2]), true)
}

/// A full pull of subjects `(number, is_pr, timeline)` in listing order.
fn full_pull(subjects: &[(u64, bool, &[TimelineEvent])]) -> Ocel {
    let mut staging = StagingLog::new();
    let mut m = new_mapper();
    m.register(&mut staging);
    for &(number, pr, timeline) in subjects {
        m.map_issue(&mut staging, &issue(number, pr), timeline, &[]);
    }
    m.resolve_closes(&mut staging, resolve_abc).unwrap();
    staging.into_ocel().expect("valid log")
}

fn refresh_set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn closes_links(log: &Ocel) -> Vec<(&str, &str)> {
    log.o2o()
        .filter(|r| r.qualifier == "closes")
        .map(|r| (r.source_id, r.target_id))
        .collect()
}

/// Refreshing only the closed issue re-resolves its closing commit onto the
/// unrefreshed PR: equal to a full re-pull, and the link is not duplicated.
#[test]
fn issue_refresh_equals_full_pull_without_duplicating_the_link() {
    let pr_tl = vec![entry("merged", 9)];
    let issue_v1 = vec![closed_by("abc123", 10)];
    let issue_v2 = vec![closed_by("abc123", 10), entry("commented", 20)];

    let v1 = full_pull(&[(2, true, &pr_tl), (1, false, &issue_v1)]);
    assert_eq!(closes_links(&v1), vec![("o/r#2", "o/r#1")]);

    let refreshed = refresh_set(&["o/r#1"]);
    let mut staging = StagingLog::from_ocel(prune_refreshed(&v1, &refreshed));
    let mut m = new_mapper();
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(1, false), &issue_v2, &[]);
    m.resolve_closes(&mut staging, resolve_abc).unwrap();
    repair_closes_links(&mut staging, &v1, &refreshed);
    let incremental = staging.into_ocel().expect("valid log");

    assert_eq!(
        incremental,
        full_pull(&[(2, true, &pr_tl), (1, false, &issue_v2)])
    );
    assert_eq!(closes_links(&incremental), vec![("o/r#2", "o/r#1")]);
}

/// Refreshing only the PR rebuilds its object from a timeline that knows
/// nothing of the close — the repair carries the link over from the
/// existing log, and nothing needs re-resolution.
#[test]
fn pr_refresh_keeps_the_link_via_repair_and_equals_full_pull() {
    let issue_tl = vec![closed_by("abc123", 10)];
    let pr_v1 = vec![entry("merged", 9)];
    let pr_v2 = vec![entry("merged", 9), entry("commented", 21)];

    let v1 = full_pull(&[(1, false, &issue_tl), (2, true, &pr_v1)]);
    assert_eq!(closes_links(&v1), vec![("o/r#2", "o/r#1")]);

    let refreshed = refresh_set(&["o/r#2"]);
    let mut staging = StagingLog::from_ocel(prune_refreshed(&v1, &refreshed));
    let mut m = new_mapper();
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(2, true), &pr_v2, &[]);
    m.resolve_closes(&mut staging, no_resolution).unwrap();
    repair_closes_links(&mut staging, &v1, &refreshed);
    let incremental = staging.into_ocel().expect("valid log");

    assert_eq!(
        incremental,
        full_pull(&[(1, false, &issue_tl), (2, true, &pr_v2)])
    );
    assert_eq!(closes_links(&incremental), vec![("o/r#2", "o/r#1")]);
}

/// Refreshing both subjects remaps and re-resolves everything from scratch.
#[test]
fn refreshing_both_subjects_equals_full_pull_with_one_link() {
    let issue_tl = vec![closed_by("abc123", 10)];
    let pr_v1 = vec![entry("merged", 9)];
    let pr_v2 = vec![entry("merged", 9), entry("commented", 21)];

    let v1 = full_pull(&[(1, false, &issue_tl), (2, true, &pr_v1)]);

    let refreshed = refresh_set(&["o/r#1", "o/r#2"]);
    let mut staging = StagingLog::from_ocel(prune_refreshed(&v1, &refreshed));
    let mut m = new_mapper();
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(1, false), &issue_tl, &[]);
    m.map_issue(&mut staging, &issue(2, true), &pr_v2, &[]);
    m.resolve_closes(&mut staging, resolve_abc).unwrap();
    repair_closes_links(&mut staging, &v1, &refreshed);
    let incremental = staging.into_ocel().expect("valid log");

    assert_eq!(
        incremental,
        full_pull(&[(1, false, &issue_tl), (2, true, &pr_v2)])
    );
    assert_eq!(closes_links(&incremental), vec![("o/r#2", "o/r#1")]);
}

/// When the link survives pruning (a kept event of the unrefreshed PR still
/// references the issue) *and* the refreshed issue re-resolves it, the two
/// identical pairs merge into a single relationship at the gate.
#[test]
fn surviving_link_plus_re_resolution_dedupes_to_one() {
    let issue_tl = vec![closed_by("abc123", 10)];
    // the PR's timeline cross-references the issue, keeping the issue
    // object — and the `closes` link pointing at it — alive through the prune
    let mut xref = entry("cross-referenced", 11);
    xref.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 1,
            repository: Some(RepoRef {
                full_name: "o/r".into(),
            }),
        }),
    });
    let pr_tl = vec![entry("merged", 9), xref];

    let v1 = full_pull(&[(2, true, &pr_tl), (1, false, &issue_tl)]);
    assert_eq!(closes_links(&v1), vec![("o/r#2", "o/r#1")]);

    let refreshed = refresh_set(&["o/r#1"]);
    let mut staging = StagingLog::from_ocel(prune_refreshed(&v1, &refreshed));
    let mut m = new_mapper();
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(1, false), &issue_tl, &[]);
    m.resolve_closes(&mut staging, resolve_abc).unwrap();
    repair_closes_links(&mut staging, &v1, &refreshed);
    let incremental = staging.into_ocel().expect("valid log");

    assert_eq!(closes_links(&incremental), vec![("o/r#2", "o/r#1")]);
}
