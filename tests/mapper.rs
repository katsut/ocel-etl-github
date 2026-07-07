use std::collections::BTreeSet;
use std::convert::Infallible;

use chrono::{DateTime, TimeZone, Utc};
use ocel::AttrValue;
use ocel_etl::StagingLog;
use ocel_etl_github::mapper::RepoMapper;
use ocel_etl_github::models::{
    Actor, CrossRefIssue, CrossRefSource, Issue, Label, PullRequestMarker, RepoRef, Review,
    TimelineEvent,
};

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

fn mapper<'a>(repo: &'a str, known: &[u64]) -> RepoMapper<'a> {
    RepoMapper::new(repo, known.iter().copied().collect::<BTreeSet<u64>>(), true)
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

/// For tests where no close awaits resolution: fails the test if called.
fn no_resolution(_sha: &str) -> Result<Vec<u64>, Infallible> {
    panic!("resolve must not be called")
}

#[test]
fn issue_lifecycle_maps_to_events_and_state() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let mut labeled = entry("labeled", 5);
    labeled.label = Some(Label { name: "bug".into() });
    let timeline = vec![
        entry("commented", 2),
        labeled,
        entry("closed", 10),
        entry("reopened", 20),
        entry("closed", 25),
    ];
    m.map_issue(&mut staging, &issue(1, false), &timeline, &[]);

    let log = staging.into_ocel().expect("valid log");
    let types: Vec<&str> = log.events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "open issue",
            "comment",
            "label",
            "close issue",
            "reopen issue",
            "close issue"
        ]
    );

    // every event links subject, actor, and repo
    for event in &log.events {
        let quals: Vec<&str> = event
            .relationships
            .iter()
            .map(|r| r.qualifier.as_str())
            .collect();
        assert!(quals.contains(&"subject"));
        assert!(quals.contains(&"actor"));
        assert!(quals.contains(&"repo"));
    }

    // dynamic state: open at creation, closed at the end
    let subject = log.objects.iter().find(|o| o.id == "o/r#1").unwrap();
    assert_eq!(subject.object_type, "issue");
    assert_eq!(
        subject.attribute_at("state", ts(1)),
        Some(&AttrValue::String("open".into()))
    );
    assert_eq!(
        subject.attribute_at("state", ts(59)),
        Some(&AttrValue::String("closed".into()))
    );

    // objects: issue + repository + alice + bob
    assert_eq!(log.objects.len(), 4);
}

#[test]
fn pull_request_gets_reviews_and_merge() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let reviews = vec![Review {
        id: 900,
        user: Some(actor("carol")),
        state: "APPROVED".into(),
        submitted_at: Some(ts(7)),
    }];
    let timeline = vec![entry("merged", 9)];
    m.map_issue(&mut staging, &issue(2, true), &timeline, &reviews);

    let log = staging.into_ocel().expect("valid log");
    let types: Vec<&str> = log.events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec!["open pull request", "merge pull request", "review"]
    );

    let review = log
        .events
        .iter()
        .find(|e| e.event_type == "review")
        .unwrap();
    assert_eq!(
        review.attributes[0].value,
        AttrValue::String("approved".into())
    );

    let subject = log.objects.iter().find(|o| o.id == "o/r#2").unwrap();
    assert_eq!(subject.object_type, "pull_request");
    assert_eq!(
        subject.attribute_at("state", ts(59)),
        Some(&AttrValue::String("merged".into()))
    );
}

#[test]
fn same_repo_cross_reference_links_and_foreign_is_counted() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    // the referencing PR must exist in the log
    m.map_issue(&mut staging, &issue(2, true), &[], &[]);

    let mut same_repo = entry("cross-referenced", 3);
    same_repo.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 2,
            repository: Some(RepoRef {
                full_name: "o/r".into(),
            }),
        }),
    });
    let mut foreign = entry("cross-referenced", 4);
    foreign.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 9,
            repository: Some(RepoRef {
                full_name: "other/repo".into(),
            }),
        }),
    });
    m.map_issue(&mut staging, &issue(1, false), &[same_repo, foreign], &[]);

    assert_eq!(
        m.skipped_kinds().get("cross-referenced (other repo)"),
        Some(&1)
    );

    let log = staging.into_ocel().expect("valid log");
    let reference = log
        .events
        .iter()
        .find(|e| e.event_type == "reference")
        .unwrap();
    assert!(reference
        .relationships
        .iter()
        .any(|r| r.object_id == "o/r#2" && r.qualifier == "referenced by"));
}

#[test]
fn same_repo_reference_to_missing_subject_is_counted_not_linked() {
    // #55 was deleted / converted to a discussion: same repo, but absent
    // from the listing and therefore not a known subject
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let mut ghost_ref = entry("cross-referenced", 3);
    ghost_ref.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 55,
            repository: Some(RepoRef {
                full_name: "o/r".into(),
            }),
        }),
    });
    m.map_issue(&mut staging, &issue(1, false), &[ghost_ref], &[]);

    assert_eq!(
        m.skipped_kinds().get("cross-referenced (missing subject)"),
        Some(&1)
    );
    let log = staging.into_ocel().expect("valid log — nothing dangles");
    assert!(log.events.iter().all(|e| e.event_type != "reference"));
}

#[test]
fn unknown_timeline_kinds_are_counted_not_dropped_silently() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);
    m.map_issue(
        &mut staging,
        &issue(1, false),
        &[entry("committed", 3), entry("committed", 4)],
        &[],
    );
    assert_eq!(m.skipped_kinds().get("committed"), Some(&2));
}

#[test]
fn comment_bodies_are_stored_when_enabled_and_omitted_when_not() {
    let mut with_body = entry("commented", 3);
    with_body.body = Some("thanks!".into());

    let mut staging = StagingLog::new();
    let mut on = mapper("o/r", &[1]);
    on.register(&mut staging);
    on.map_issue(&mut staging, &issue(1, false), &[with_body.clone()], &[]);
    let log = staging.into_ocel().expect("valid log");
    let comment = log
        .events
        .iter()
        .find(|e| e.event_type == "comment")
        .unwrap();
    assert_eq!(
        comment.attributes[0].value,
        AttrValue::String("thanks!".into())
    );

    let mut staging = StagingLog::new();
    let mut off = RepoMapper::new("o/r", [1].into_iter().collect::<BTreeSet<u64>>(), false);
    off.register(&mut staging);
    off.map_issue(&mut staging, &issue(1, false), &[with_body], &[]);
    let log = staging.into_ocel().expect("valid log");
    let comment = log
        .events
        .iter()
        .find(|e| e.event_type == "comment")
        .unwrap();
    assert!(comment.attributes.is_empty());
}

#[test]
fn issues_closed_by_a_pr_commit_get_closes_o2o_on_the_pr() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2, 3]);
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(3, true), &[entry("merged", 9)], &[]);
    // one squash-merge commit closed both issues
    m.map_issue(
        &mut staging,
        &issue(1, false),
        &[closed_by("abc123", 10)],
        &[],
    );
    m.map_issue(
        &mut staging,
        &issue(2, false),
        &[closed_by("abc123", 11)],
        &[],
    );

    let mut lookups = 0;
    m.resolve_closes(&mut staging, |sha| {
        lookups += 1;
        assert_eq!(sha, "abc123");
        Ok::<_, Infallible>(vec![3])
    })
    .unwrap();
    assert_eq!(lookups, 1, "distinct shas are resolved once");
    assert!(m.skipped_kinds().is_empty(), "{:?}", m.skipped_kinds());

    let log = staging.into_ocel().expect("valid log");
    // the link sits on the pull_request object, pointing at the issues
    let pr = log.objects.iter().find(|o| o.id == "o/r#3").unwrap();
    assert!(pr
        .relationships
        .iter()
        .any(|r| r.object_id == "o/r#1" && r.qualifier == "closes"));
    assert!(pr
        .relationships
        .iter()
        .any(|r| r.object_id == "o/r#2" && r.qualifier == "closes"));
    let subject = log.objects.iter().find(|o| o.id == "o/r#1").unwrap();
    assert!(subject.relationships.is_empty());
}

#[test]
fn close_attributed_via_same_instant_referenced_commit_is_linked() {
    // a keyword close from a merged PR leaves `closed.commit_id` null and
    // writes a `referenced` entry for the closing commit at the same instant
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(2, true), &[entry("merged", 9)], &[]);

    let mut referenced = entry("referenced", 10);
    referenced.commit_id = Some("abc123".into());
    m.map_issue(
        &mut staging,
        &issue(1, false),
        &[referenced, entry("closed", 10)],
        &[],
    );

    let mut lookups = 0;
    m.resolve_closes(&mut staging, |sha| {
        lookups += 1;
        assert_eq!(sha, "abc123");
        Ok::<_, Infallible>(vec![2])
    })
    .unwrap();
    assert_eq!(lookups, 1);
    assert_eq!(m.skipped_kinds().get("closed (unlinked commit)"), None);

    let log = staging.into_ocel().expect("valid log");
    let pr = log.objects.iter().find(|o| o.id == "o/r#2").unwrap();
    assert!(pr
        .relationships
        .iter()
        .any(|r| r.object_id == "o/r#1" && r.qualifier == "closes"));
}

#[test]
fn reference_at_another_moment_does_not_attribute_a_close() {
    // a commit referenced the issue 2 minutes before someone closed it by
    // hand: not the closer, no link, no counter
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let mut referenced = entry("referenced", 8);
    referenced.commit_id = Some("abc123".into());
    m.map_issue(
        &mut staging,
        &issue(1, false),
        &[referenced, entry("closed", 10)],
        &[],
    );

    m.resolve_closes(&mut staging, no_resolution).unwrap();
    assert_eq!(m.skipped_kinds().get("closed (unlinked commit)"), None);

    let log = staging.into_ocel().expect("valid log");
    assert!(log.objects.iter().all(|o| o.relationships.is_empty()));
}

#[test]
fn manual_close_needs_no_resolution_and_no_counter() {
    // closed by hand: no commit_id on the event — that is normal
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1]);
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(1, false), &[entry("closed", 10)], &[]);

    m.resolve_closes(&mut staging, no_resolution).unwrap();
    assert!(m.skipped_kinds().is_empty(), "{:?}", m.skipped_kinds());

    let log = staging.into_ocel().expect("valid log");
    assert!(log.objects.iter().all(|o| o.relationships.is_empty()));
}

#[test]
fn unresolvable_closing_commit_is_counted_not_guessed() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);
    m.map_issue(&mut staging, &issue(1, false), &[closed_by("aaa", 10)], &[]);
    m.map_issue(&mut staging, &issue(2, false), &[closed_by("bbb", 12)], &[]);

    m.resolve_closes(&mut staging, |sha| {
        Ok::<_, Infallible>(match sha {
            "aaa" => vec![], // no associated pull request at all
            _ => vec![99],   // a PR outside the pulled set
        })
    })
    .unwrap();
    assert_eq!(m.skipped_kinds().get("closed (unlinked commit)"), Some(&2));

    let log = staging.into_ocel().expect("valid log — nothing dangles");
    assert!(log.objects.iter().all(|o| o.relationships.is_empty()));
}

#[test]
fn a_merged_prs_own_close_commit_is_not_resolved() {
    // a merged PR's timeline carries `closed` with the merge commit sha;
    // resolving it would only ever find the PR itself
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[2]);
    m.register(&mut staging);
    m.map_issue(
        &mut staging,
        &issue(2, true),
        &[entry("merged", 9), closed_by("abc123", 10)],
        &[],
    );

    m.resolve_closes(&mut staging, no_resolution).unwrap();
    assert!(m.skipped_kinds().is_empty(), "{:?}", m.skipped_kinds());
    staging.into_ocel().expect("valid log");
}
