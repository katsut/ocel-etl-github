use std::cell::RefCell;

use ocel_etl_github::client::{ClientError, GithubClient, HttpGet, HttpResponse};

/// Scripted transport: pops one canned result per request, recording URLs.
struct FakeHttp {
    responses: RefCell<Vec<Result<HttpResponse, ClientError>>>,
    urls: RefCell<Vec<String>>,
}

impl FakeHttp {
    fn new(responses: Vec<Result<HttpResponse, ClientError>>) -> Self {
        Self {
            responses: RefCell::new(responses),
            urls: RefCell::new(Vec::new()),
        }
    }
}

impl HttpGet for &FakeHttp {
    fn get(&self, url: &str) -> Result<HttpResponse, ClientError> {
        self.urls.borrow_mut().push(url.to_owned());
        self.responses.borrow_mut().remove(0)
    }
}

#[allow(clippy::unnecessary_wraps)] // scripted responses are Results by design
fn ok(body: &str) -> Result<HttpResponse, ClientError> {
    Ok(HttpResponse {
        status: 200,
        retry_after: None,
        body: body.to_owned(),
    })
}

fn issue_json(number: u64) -> String {
    format!(
        r#"{{"number":{number},"title":"t","state":"open","user":null,
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}}"#
    )
}

#[test]
fn pagination_stops_on_short_page() {
    let full_page: String = format!(
        "[{}]",
        (1..=100).map(issue_json).collect::<Vec<_>>().join(",")
    );
    let fake = FakeHttp::new(vec![ok(&full_page), ok(&format!("[{}]", issue_json(101)))]);
    let client = GithubClient::new(&fake, "http://fake");

    let issues = client.issues("o/r", None).expect("two pages");
    assert_eq!(issues.len(), 101);

    let urls = fake.urls.borrow();
    assert_eq!(urls.len(), 2);
    assert!(urls[0].contains("page=1"));
    assert!(urls[1].contains("page=2"));
    assert!(urls[0].contains("/repos/o/r/issues?"));
}

#[test]
fn commit_pulls_hits_the_commit_endpoint_and_parses() {
    let fake = FakeHttp::new(vec![ok(r#"[{"number":292},{"number":300}]"#)]);
    let client = GithubClient::new(&fake, "http://fake");

    let pulls = client.commit_pulls("o/r", "f01685c").expect("parsed");
    let numbers: Vec<u64> = pulls.iter().map(|p| p.number).collect();
    assert_eq!(numbers, vec![292, 300]);
    let urls = fake.urls.borrow();
    assert_eq!(urls.len(), 1, "one short page suffices");
    assert!(urls[0].contains("/repos/o/r/commits/f01685c/pulls?"));
}

#[test]
fn rate_limit_is_retried_then_succeeds() {
    let limited = Ok(HttpResponse {
        status: 429,
        retry_after: Some(0),
        body: String::new(),
    });
    let fake = FakeHttp::new(vec![limited, ok("[]")]);
    let client = GithubClient::new(&fake, "http://fake");
    assert!(client.timeline("o/r", 1).expect("retried").is_empty());
}

#[test]
fn transient_transport_error_is_retried() {
    let fake = FakeHttp::new(vec![
        Err(ClientError::Transport("connection reset".into())),
        ok("[]"),
    ]);
    let client = GithubClient::new(&fake, "http://fake");
    assert!(client.timeline("o/r", 715).expect("retried").is_empty());
    assert_eq!(fake.urls.borrow().len(), 2);
}

#[test]
fn permanent_transport_failure_gives_up_with_the_original_error() {
    let responses = (0..=5)
        .map(|_| Err(ClientError::Transport("connection reset".into())))
        .collect();
    let fake = FakeHttp::new(responses);
    let client = GithubClient::new(&fake, "http://fake");
    let err = client.timeline("o/r", 1).expect_err("gives up");
    assert!(matches!(err, ClientError::Transport(_)), "{err}");
    assert_eq!(fake.urls.borrow().len(), 6);
}

#[test]
fn non_rate_limit_status_fails_immediately() {
    let fake = FakeHttp::new(vec![Ok(HttpResponse {
        status: 404,
        retry_after: None,
        body: String::new(),
    })]);
    let client = GithubClient::new(&fake, "http://fake");
    let err = client.issues("o/missing", None).expect_err("404 is fatal");
    assert!(
        matches!(err, ClientError::Status { status: 404, .. }),
        "{err}"
    );
    assert_eq!(fake.urls.borrow().len(), 1);
}
