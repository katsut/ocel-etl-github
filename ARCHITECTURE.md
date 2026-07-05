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

## Event ids and incremental sync

Event ids are `<subject>|<kind>` (e.g. `o/r#12|open`, `o/r#12|t123`) — `|`
cannot appear in repository names, so an incremental run prunes a refreshed
subject's old events by id prefix, remaps it, and passes the merged result
through the same validity gate as a full pull. When `--out` exists, only
issues updated since its newest event are re-fetched.

## Client

The HTTP layer sits behind a small `HttpGet` trait, so pagination and backoff
are tested without a network. Rate limits (429, or 403 with reset headers)
and transient transport failures are retried with capped, logged backoff —
waits never exceed 120s and every retry is visible on stderr. `GITHUB_TOKEN`
is optional: public repositories work anonymously.
