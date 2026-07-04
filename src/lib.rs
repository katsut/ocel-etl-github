//! GitHub → OCEL 2.0 extraction.
//!
//! Objects: `issue`, `pull_request`, `user`, `repository`. Events come from
//! the issue timeline (open / comment / label / assign / review / close /
//! merge / reopen / reference), every one linking its subject, actor, and
//! repository. See `docs/adr/0001-github-to-ocel-mapping.md`.

pub mod client;
pub mod mapper;
pub mod models;
pub mod sync;
