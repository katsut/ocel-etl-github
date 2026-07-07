# Architecture

How the GitHub connector maps development history to OCEL 2.0.

## Objects

| Object type | id | Notes |
|---|---|---|
| `issue` | `owner/repo#123` | dynamic attributes `state`, `title` |
| `pull_request` | `owner/repo#456` | same number space as issues (GitHub's model); `state` is open/closed/merged |
| `user` | `@login` | the actor of every event; `@ghost` when GitHub no longer knows the author |
| `repository` | `owner/repo` | one per pulled repo; every event links it |

## Events

From the issues timeline API plus the PR reviews API, named in plain verbs:
`open issue` / `open pull request`, `comment`, `label` / `unlabel`,
`assign` / `unassign`, `review` (state as attribute), `close …`,
`merge pull request`, `reopen …`, and `reference` for same-repo
cross-references. Every event carries three E2O links: the subject
(qualifier `subject`), the actor (`actor`), and the repository (`repo`).

Timeline kinds that are not modeled are **counted, never silently dropped** —
the CLI prints `skipped timeline kinds: mentioned x1207, …` so you can see
what the log does not contain.

Cross-references link only *known subjects*: the current listing plus, on
incremental runs, subjects already in the log. Same-repo numbers can be
absent (issues deleted, transferred, or converted to discussions keep their
number in reference sources but never appear in the listing); those are
counted as `cross-referenced (missing subject)`.

## Object relationships

A pull request whose merge closed an issue carries an O2O relationship
`closes` (source: the `pull_request` object, target: the closed `issue`).
The REST timeline never names the closing PR directly — `connected` events
carry no source reference — so the link goes through the closing *commit*:
either `closed.commit_id` (set when GitHub attributed the close to a
commit directly), or the commit of a `referenced` entry written in the
same atomic close operation, i.e. with a timestamp identical to the
`closed` entry's (keyword closes from merged PRs usually leave
`closed.commit_id` null and write that `referenced` record instead; the
same-instant rule was validated against the GraphQL ground truth,
`ClosedEvent.closer` — references written at any other moment are not
attributions and produce no link). The commit is resolved through
`GET /commits/{sha}/pulls` (one request per distinct closing commit,
cached by sha within a run) and linked only to pull requests among the
pulled subjects — a commit that resolves to no known PR (a direct push, a
force-pushed-away sha, or a foreign PR) is counted as
`closed (unlinked commit)`, never guessed, never dangling. Closes that
REST does not attribute at all (some PR closes are only attributed in
GraphQL) are indistinguishable from manual closes and produce no link.
Lifting the link into events (e.g. a `fixed by` event on the issue) is the
transform layer's job; the connector records only the structural fact.

## Event ids and incremental sync

Event ids are `<subject>|<kind>` (e.g. `o/r#12|open`, `o/r#12|t123`) — `|`
cannot appear in repository names, so an incremental run prunes a refreshed
subject's old events by id prefix, remaps it, and passes the merged result
through the same validity gate as a full pull. When `--out` exists, only
issues updated since its newest event are re-fetched.

`closes` links live on PR objects but are learned from issue timelines, so
pruning a refreshed PR would lose links contributed by issues outside the
refresh set; `repair_closes_links` re-adds those pairs from the existing
log (the same shape as the Backlog connector's parent-link repair).

## Client

The HTTP layer sits behind a small `HttpGet` trait, so pagination and backoff
are tested without a network. Rate limits (429, or 403 with reset headers)
and transient transport failures are retried with capped, logged backoff —
waits never exceed 120s and every retry is visible on stderr. `GITHUB_TOKEN`
is optional: public repositories work anonymously.
