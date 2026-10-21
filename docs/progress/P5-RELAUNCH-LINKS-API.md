# Relaunch link store — frozen interface between L6 and L7 — 2026-10-21

Parent-owned contract so the relaunch (L6) and link store (L7) workers can
run in parallel. `src/recovery_links.rs` (L7) MUST expose exactly:

```rust
pub const MAX_LINKS: usize = 64;               // oldest pruned first
pub const LINK_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOutcome { Applied, Unknown }       // only verified new targets

#[derive(Clone, Copy, Eq, PartialEq)]          // redacted Debug
pub struct RelaunchLink {
    pub source_generation: u64,                  // saved layout's generation
    pub source_sequence: u64,                    // saved layout's snapshot sequence
    pub old_target: TargetRef,                   // historical, never dispatched
    pub new_target: TargetRef,                   // new generation, owner-issued
    pub outcome: LinkOutcome,
    pub linked_at_unix_ms: u64,
}

pub struct LinkStore { .. }                      // redacted Debug
impl LinkStore {
    /// Open existing `server/links/` (0700, private); never creates.
    pub fn open(path: &Path) -> io::Result<Self>;
    /// Create `server/links/` 0700 if absent, then open.
    pub fn prepare(server_directory: &Path) -> io::Result<Self>;
    /// Validate (new_target.generation != old_target.generation, nonzero
    /// fields, workspace scope equal), append atomically (temp + fsync +
    /// fullfsync + rename + dir sync), idempotent for an identical link,
    /// refuse a conflicting link for the same old_target+new generation.
    /// Prunes entries older than LINK_RETENTION_MS relative to `now` and
    /// keeps at most MAX_LINKS. Caller holds the server singleton.
    pub fn record(&self, link: RelaunchLink, now_unix_ms: u64) -> io::Result<()>;
    /// Read-only: all valid links (whole-file refusal on corruption).
    pub fn load(&self) -> io::Result<Vec<RelaunchLink>>;
    /// Read-only filter by old target.
    pub fn links_for_old(&self, old: TargetRef) -> io::Result<Vec<RelaunchLink>>;
}
```

One file `server/links/links.bin`: fixed header magic `PLLNK001` + count,
then fixed-size records (no variable fields, no cwd/env/argv/provider data).
Links are historical associations only; they never authorize dispatch.
