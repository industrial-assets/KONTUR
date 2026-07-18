# Kontur Completion Implementation Plan (v0.1 product)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete Kontur v0.1: blocked-gate audit records, real git effects, the networked two-seat attach layer, the wired console interactions, a real-agent MCP endpoint, and the `kontur host|join|demo` binary.

**Architecture:** `kontur-core` gains `Outcome::Blocked` + serde on its sealing-safe projections; `kontur-mcp` emits audit records for blocked gates and gains a `GitWorkspace`; a new `kontur-net` crate carries a JSON-lines protocol (any `AsyncRead+AsyncWrite`; TCP in production, duplex in tests) with a `SessionServer` (session state machine over the `GateHost`) and `SessionClient` (client-side signing); `kontur-tui` gains a remote mode with wired no-go/diff/hand-edit/ready; the `kontur` binary ties it together.

**Tech Stack:** Rust 2021, tokio, serde/serde_json, rmcp 2.2 (existing), ratatui 0.30 (existing), git via `std::process::Command`.

## Global Constraints

- **No AI co-author trailers or "Generated with" footers on ANY commit** (global user rule — overrides all prior plan templates).
- Blind sealing never violated anywhere, including the wire: snapshots carry `kontur-core`'s `VerdictView` only; `SealedVerdict` must never become serializable; never log/serialize a private key or sealed value.
- No wall-clock/RNG inside `kontur-core` (net/tui layers may read system time for their own I/O concerns).
- No `HashMap`/`HashSet` in anything fed to `canonical_bytes`.
- `accept_task` reachable only from a `Satisfied` hold; `merge_session` only at session close; park-on-loss never degrades to one key.
- Stage ONLY the files each task changed — never `git add -A`/`git add .` (`/target` is gitignored but stage explicitly).
- `cargo clippy --all-targets -- -D warnings` clean; pristine test output; edition 2021.

---

## File structure

```
crates/kontur-core/src/policy.rs        # T1: Outcome::Blocked
crates/kontur-core/src/audit/record.rs  # T1: build() for resolved holds; HoldUnresolved
crates/kontur-core/src/sealed.rs        # T1: serde on VerdictStatus/VerdictView
crates/kontur-mcp/src/gatehost.rs       # T2: blocked-path audit; T3: merge_session/session trailer helpers
crates/kontur-mcp/src/workspace.rs      # T3: merge_session on the port + InMemory impl
crates/kontur-mcp/src/fs_workspace.rs   # T3: merge_session recording impl
crates/kontur-mcp/src/git_workspace.rs  # T3: GitWorkspace (new)
crates/kontur-net/                      # T4 protocol+codec; T5 server+agent; T6 client; T8 e2e test
crates/kontur-tui/src/view.rs           # T7: Prompt/Plan regions
crates/kontur-tui/src/render.rs         # T7: new arms + diff pane + close copy
crates/kontur-tui/src/remote.rs         # T7: WireState -> SessionView + remote loop
crates/kontur-tui/src/bin/kontur.rs     # T8: host|join|demo
CLAUDE.md / README.md                   # T8: docs move with behaviour
```

---

### Task 1: kontur-core — Outcome::Blocked, resolved-hold records, wire-safe serde

**Files:**
- Modify: `crates/kontur-core/src/policy.rs`
- Modify: `crates/kontur-core/src/audit/record.rs`
- Modify: `crates/kontur-core/src/sealed.rs`
- Modify: `crates/kontur-core/src/audit/chain.rs` (tests only, if needed for imports)

**Interfaces:**
- Produces: `Outcome::Blocked`; `GateRecord::build` accepting Satisfied OR Blocked holds; `RecordError::HoldUnresolved` (rename of `HoldNotSatisfied`); `Serialize`/`Deserialize` on `VerdictStatus` + `VerdictView`.

- [ ] **Step 1: Add the variant** — in `policy.rs`, extend `Outcome`:

```rust
/// How a resolved gate concluded.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Outcome {
    Unanimous,
    ResolvedAfterDisagreement,
    /// The gate resolved with a no-go; the dissenting checker entry carries the
    /// remedy and its signature. The task routed to intervention.
    Blocked,
}
```

- [ ] **Step 2: Accept resolved holds in `GateRecord::build`** — in `audit/record.rs`, replace the guard + outcome extraction:

```rust
        let outcome = match hold.state() {
            HoldState::Satisfied => hold.outcome().expect("satisfied hold has an outcome"),
            HoldState::Blocked => Outcome::Blocked,
            _ => return Err(RecordError::HoldUnresolved),
        };
```

Rename the error variant (and its message):

```rust
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum RecordError {
    #[error("cannot record an unresolved gate")]
    HoldUnresolved,
}
```

Update the build doc comment: a record is built for any **resolved** hold; a blocked
record's checker entries carry the `NoGo(Remedy)` verdict + signature, so the
dissent and remedy are chained with no new fields. Update the existing
`refuses_unsatisfied_hold` test to the new name (`RecordError::HoldUnresolved`).

- [ ] **Step 3: Serde on the sealing-safe projections** — in `sealed.rs`, add `use serde::{Deserialize, Serialize};` and extend the derives:

```rust
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum VerdictStatus { ... }   // unchanged variants

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct VerdictView { ... }   // unchanged fields
```

`SealedVerdict` must NOT gain serde. Add a doc note on `VerdictView`: safe to
serialize — `Sealed` is data-free by construction.

- [ ] **Step 4: Tests** — add to `audit/record.rs` tests:

```rust
    fn blocked_hold() -> DualHold {
        let mut h = DualHold::new(
            GateId("g1".into()), TaskId("t1".into()), Hash([9u8; 32]),
            GatePolicy::default(), MakerSet::new(), Authorship::Agent,
        );
        let s1 = Ed25519Signer::from_seed([1; 32]);
        let cv = CastVerdict::create(&s1, &FixedClock(1000), h.gate_id(), h.diff_hash(),
            Verdict::Go, ReviewDepth::FullDiff, None);
        h.cast(0, cv).unwrap();
        let s2 = Ed25519Signer::from_seed([2; 32]);
        let cv = CastVerdict::create(&s2, &FixedClock(1001), h.gate_id(), h.diff_hash(),
            Verdict::NoGo(crate::Remedy::Steer("cache it".into())), ReviewDepth::FullDiff, None);
        h.cast(1, cv).unwrap();
        h
    }

    #[test]
    fn blocked_hold_builds_record_with_remedy_and_outcome() {
        let h = blocked_hold();
        let rec = GateRecord::build(Hash([0u8; 32]), provenance(), &h).unwrap();
        assert_eq!(rec.core.outcome, Outcome::Blocked);
        assert!(rec.core.checkers.iter().any(|c| matches!(
            &c.verdict, Verdict::NoGo(crate::Remedy::Steer(s)) if s == "cache it")));
        assert_eq!(rec.this_hash, rec.recompute_hash());
    }

    #[test]
    fn open_hold_still_refused() {
        let h = DualHold::new(GateId("g2".into()), TaskId("t2".into()), Hash([1u8; 32]),
            GatePolicy::default(), MakerSet::new(), Authorship::Agent);
        assert_eq!(GateRecord::build(Hash([0u8; 32]), provenance(), &h).unwrap_err(),
            RecordError::HoldUnresolved);
    }
```

Add to `audit/chain.rs` tests (a mixed chain verifies; reuse the existing `record()` helper and the new blocked-record construction inline):

```rust
    #[test]
    fn chain_with_blocked_record_verifies_and_detects_tamper() {
        let mut chain = AuditChain::new();
        chain.append(record(GENESIS, "g1")).unwrap();
        // blocked record chained after the satisfied one
        let h = {
            let mut h = DualHold::new(GateId("g2".into()), TaskId("t2".into()), Hash([9u8; 32]),
                GatePolicy::default(), MakerSet::new(), Authorship::Agent);
            for (seed, v) in [(1u8, Verdict::Go),
                (2u8, Verdict::NoGo(crate::Remedy::Steer("fix".into())))] {
                let s = Ed25519Signer::from_seed([seed; 32]);
                let cv = CastVerdict::create(&s, &FixedClock(1000 + seed as i64), h.gate_id(),
                    h.diff_hash(), v, ReviewDepth::FullDiff, None);
                let ev = h.version();
                h.cast(ev, cv).unwrap();
            }
            h
        };
        let prov = Provenance {
            task_id: TaskId("t2".into()),
            prompt: "p".into(),
            prompt_author: Ed25519Signer::from_seed([1; 32]).operator_id(),
            agent_id: "a".into(),
            agent_model: "m".into(),
            agent_version: "v".into(),
            diff_hash: Hash([9u8; 32]),
            files: vec!["f".into()],
            loc: 1,
            tokens: 1,
        };
        let rec = GateRecord::build(chain.head(), prov, &h).unwrap();
        chain.append(rec).unwrap();
        assert!(verify_chain(chain.records()).is_ok());
        let mut tampered = chain.records().to_vec();
        tampered[1].core.outcome = Outcome::Unanimous; // lie about the outcome
        assert!(verify_chain(&tampered).is_err());
    }
```

(Construct `prov` inline exactly like the existing `record()` helper's provenance with `task_id: TaskId("t2".into())`.)

Add to `sealed.rs` tests:

```rust
    #[test]
    fn sealed_view_serializes_without_verdict_value() {
        let cv = a_cast(); // existing helper, a Go verdict
        let sv = SealedVerdict::new(cv, true);
        let json = serde_json::to_string(&sv.view()).unwrap();
        assert!(json.contains("Sealed"));
        assert!(!json.contains("Revealed"));
        assert!(!json.contains("\"Go\""));
    }
```

Add `serde_json = "1"` to `crates/kontur-core/Cargo.toml` `[dev-dependencies]`.

- [ ] **Step 5: Run** — `cargo test -p kontur-core` (all pass incl. new), then `cargo test` (workspace — the tui/mcp crates must still compile against the renamed error; nothing else references it), then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 6: Commit** — `git add crates/kontur-core/src/policy.rs crates/kontur-core/src/audit/record.rs crates/kontur-core/src/audit/chain.rs crates/kontur-core/src/sealed.rs crates/kontur-core/Cargo.toml Cargo.lock` ; message: `feat(core): blocked-gate audit records + wire-safe verdict views` (NO co-author trailer).

---

### Task 2: kontur-mcp — audit the blocked path

**Files:**
- Modify: `crates/kontur-mcp/src/gatehost.rs`

**Interfaces:**
- Produces: `submit_verdict`'s `Blocked` branch appends a `GateRecord` (with `Outcome::Blocked`) to the chain before discarding; `audit_len` counts it.

- [ ] **Step 1: Emit the record** — replace the `HoldState::Blocked` arm in `submit_verdict`:

```rust
            HoldState::Blocked => {
                let prev = st.chain.head();
                let (task_id, remedy, record) = {
                    let e = &st.holds[idx];
                    let rec = GateRecord::build(prev, e.provenance.clone(), &e.hold)
                        .expect("a resolved hold always builds a record");
                    (e.hold.task_id().clone(), e.hold.blocking_remedy(), rec)
                };
                st.chain.append(record).expect("chain head matches prev by construction");
                self.workspace.discard_task(&task_id)?;
                remedy
            }
```

(The record is appended first: the gate *did* resolve; a workspace discard failure
is surfaced but cannot un-resolve the decision.)

- [ ] **Step 2: Tests** — extend the existing `nogo_blocks_discards_and_returns_remedy` test with:

```rust
        assert_eq!(host.audit_len().await, 1);
        assert!(host.verify_audit().await.is_ok());
```

And add:

```rust
    #[tokio::test]
    async fn blocked_then_reworked_satisfied_chains_two_records() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());

        let (gid, dh) = open_a_gate(&host, &ws, &context).await;
        host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        host.submit_verdict(&gid, nogo_verdict(2, &gid, dh, "cache it")).await.unwrap();
        assert_eq!(host.audit_len().await, 1);

        // Rework: new write, fresh gate, both go.
        let task = TaskId("t1".into());
        ws.apply_write(&task, "a.rs", b"reworked\n").unwrap();
        let (gid2, _rx) = host.begin_task_gate(task, 0).await.unwrap();
        let dh2 = host.pending_gates().await[0].diff_hash;
        host.submit_verdict(&gid2, go_verdict(1, &gid2, dh2)).await.unwrap();
        host.submit_verdict(&gid2, go_verdict(2, &gid2, dh2)).await.unwrap();

        assert_eq!(host.audit_len().await, 2);
        assert!(host.verify_audit().await.is_ok());
    }
```

- [ ] **Step 3: Run** — `cargo test -p kontur-mcp` (all pass), `cargo clippy -p kontur-mcp --all-targets -- -D warnings`.

- [ ] **Step 4: Commit** — `git add crates/kontur-mcp/src/gatehost.rs` ; message: `feat(mcp): emit audit records for blocked gates` (no trailer).

---

### Task 3: kontur-mcp — merge_session port + GitWorkspace + GateHost pass-throughs

**Files:**
- Modify: `crates/kontur-mcp/src/workspace.rs`
- Modify: `crates/kontur-mcp/src/fs_workspace.rs`
- Create: `crates/kontur-mcp/src/git_workspace.rs`
- Modify: `crates/kontur-mcp/src/gatehost.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`

**Interfaces:**
- Produces: `Workspace::merge_session(&self, message: &str) -> Result<(), WorkspaceError>`; `InMemoryWorkspace::merged_message() -> Option<String>`; `GitWorkspace::create(repo: PathBuf, session: &str) -> Result<Self, WorkspaceError>` implementing `Workspace`; `GateHost::merge_session(&self, message: &str)` pass-through; `GateHost::session_operators() -> Vec<OperatorId>` (from `SessionContext.operators`, for trailer composition).

- [ ] **Step 1: Extend the port** — in `workspace.rs`, add to the `Workspace` trait:

```rust
    /// Session-end effect: land the approved session as one reviewed commit
    /// (real impls squash-merge the session branch; test doubles record it).
    /// Reachable only at session close.
    fn merge_session(&self, message: &str) -> Result<(), WorkspaceError>;
```

`InMemoryWorkspace`: add `merged: Option<String>` to `Inner`; implement
`merge_session` storing the message; add `pub fn merged_message(&self) -> Option<String>`.
Test: `merge_session_records_message`.

- [ ] **Step 2: FsWorkspace** — implement `merge_session` recording into a
`Mutex<Option<String>>` field with a `merged_message()` accessor (Fs has no repo;
the real git effect lives in `GitWorkspace`). One test.

- [ ] **Step 3: GitWorkspace** — create `git_workspace.rs`:

```rust
use std::path::PathBuf;
use std::process::Command;

use kontur_core::TaskId;

use crate::error::WorkspaceError;
use crate::workspace::{CommandOutput, FrozenDiff, Workspace};

/// Real git effects. The session lives on branch `kontur/<session>` in a
/// dedicated worktree (under the system temp dir), leaving the user's checkout
/// untouched until `merge_session` squash-merges into the original branch.
/// Requires: the target repo's checked-out branch is clean at merge time.
pub struct GitWorkspace {
    repo: PathBuf,
    worktree: PathBuf,
    branch: String,
    base: String,
}

fn git(dir: &std::path::Path, args: &[&str]) -> Result<String, WorkspaceError> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()
        .map_err(|e| WorkspaceError::Io(e.to_string()))?;
    if !out.status.success() {
        return Err(WorkspaceError::Io(format!(
            "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr))));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl GitWorkspace {
    pub fn create(repo: PathBuf, session: &str) -> Result<Self, WorkspaceError> {
        let base = git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
        let branch = format!("kontur/{session}");
        let mut worktree = std::env::temp_dir();
        worktree.push(format!("kontur-wt-{session}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&worktree);
        git(&repo, &["worktree", "add", worktree.to_str().unwrap(), "-b", &branch])?;
        Ok(GitWorkspace { repo, worktree, branch, base })
    }

    pub fn branch(&self) -> &str { &self.branch }
}

impl Workspace for GitWorkspace {
    fn apply_write(&self, _task_id: &TaskId, path: &str, contents: &[u8]) -> Result<(), WorkspaceError> {
        let full = self.worktree.join(path);
        if let Some(p) = full.parent() {
            std::fs::create_dir_all(p).map_err(|e| WorkspaceError::Io(e.to_string()))?;
        }
        std::fs::write(&full, contents).map_err(|e| WorkspaceError::Io(e.to_string()))
    }

    fn run_command(&self, _task_id: &TaskId, command: &str, cwd: &str) -> Result<CommandOutput, WorkspaceError> {
        let dir = if cwd.is_empty() { self.worktree.clone() } else { self.worktree.join(cwd) };
        let out = Command::new("sh").arg("-c").arg(command).current_dir(&dir).output()
            .map_err(|e| WorkspaceError::Io(e.to_string()))?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
        })
    }

    /// Stage everything, then freeze the staged diff vs HEAD — the exact bytes
    /// the operators review and sign against.
    fn freeze_task_diff(&self, task_id: &TaskId) -> Result<FrozenDiff, WorkspaceError> {
        git(&self.worktree, &["add", "-A"])?;
        let bytes = git(&self.worktree, &["diff", "--cached"])?.into_bytes();
        if bytes.is_empty() {
            return Err(WorkspaceError::UnknownTask(task_id.0.clone()));
        }
        let numstat = git(&self.worktree, &["diff", "--cached", "--numstat"])?;
        let mut files = Vec::new();
        let mut loc = 0u32;
        for line in numstat.lines() {
            let mut parts = line.split_whitespace();
            let adds = parts.next().unwrap_or("0");
            let _dels = parts.next();
            if let Some(name) = parts.next() {
                loc += adds.parse::<u32>().unwrap_or(0);
                files.push(name.to_string());
            }
        }
        Ok(FrozenDiff { bytes, files, loc })
    }

    fn accept_task(&self, task_id: &TaskId) -> Result<(), WorkspaceError> {
        git(&self.worktree, &["add", "-A"])?;
        git(&self.worktree, &["commit", "-m", &format!("kontur: task {}", task_id.0)])?;
        Ok(())
    }

    fn discard_task(&self, _task_id: &TaskId) -> Result<(), WorkspaceError> {
        git(&self.worktree, &["reset", "--hard", "HEAD"])?;
        git(&self.worktree, &["clean", "-fd"])?;
        Ok(())
    }

    fn merge_session(&self, message: &str) -> Result<(), WorkspaceError> {
        git(&self.repo, &["merge", "--squash", &self.branch])?;
        git(&self.repo, &["commit", "-m", message])?;
        git(&self.repo, &["worktree", "remove", "--force", self.worktree.to_str().unwrap()])?;
        Ok(())
    }
}
```

Tests (module `#[cfg(test)]` in the same file) against temp repos:

```rust
    fn temp_repo() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!("kontur-git-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        let run = |args: &[&str]| { git(&p, args).unwrap(); };
        git(&p, &["init", "-b", "main"]).unwrap();
        run(&["config", "user.email", "test@kontur.local"]);
        run(&["config", "user.name", "Kontur Test"]);
        std::fs::write(p.join("README.md"), "seed\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-m", "seed"]);
        p
    }

    #[test]
    fn freeze_accept_and_merge_with_trailers() {
        let repo = temp_repo();
        let ws = GitWorkspace::create(repo.clone(), "s1").unwrap();
        let t = TaskId("t1".into());
        ws.apply_write(&t, "src/lib.rs", b"pub fn f() {}\n").unwrap();
        let frozen = ws.freeze_task_diff(&t).unwrap();
        assert_eq!(frozen.files, vec!["src/lib.rs".to_string()]);
        assert!(frozen.loc >= 1);
        ws.accept_task(&t).unwrap();
        ws.merge_session("kontur session s1\n\nReviewed-by: A <a>\nReviewed-by: B <b>").unwrap();
        let log = git(&repo, &["log", "-1", "--format=%B", "main"]).unwrap();
        assert!(log.contains("Reviewed-by: A <a>"));
        assert!(log.contains("Reviewed-by: B <b>"));
        let count = git(&repo, &["rev-list", "--count", "main"]).unwrap();
        assert_eq!(count.trim(), "2"); // seed + one squash commit
    }

    #[test]
    fn discard_resets_worktree() {
        let repo = temp_repo();
        let ws = GitWorkspace::create(repo, "s2").unwrap();
        let t = TaskId("t1".into());
        ws.apply_write(&t, "junk.txt", b"x\n").unwrap();
        let _ = ws.freeze_task_diff(&t).unwrap();
        ws.discard_task(&t).unwrap();
        assert!(ws.freeze_task_diff(&t).is_err()); // nothing left to review
    }

    #[test]
    fn empty_diff_is_an_error() {
        let repo = temp_repo();
        let ws = GitWorkspace::create(repo, "s3").unwrap();
        assert!(ws.freeze_task_diff(&TaskId("t".into())).is_err());
    }
```

- [ ] **Step 4: GateHost pass-throughs** — in `gatehost.rs`, add:

```rust
    /// Session-end: land the approved work as one reviewed commit.
    pub async fn merge_session(&self, message: &str) -> Result<(), GateHostError> {
        Ok(self.workspace.merge_session(message)?)
    }

    /// The session's operator roster (for composing Reviewed-by trailers).
    pub async fn session_operators(&self) -> Vec<OperatorId> {
        self.state.lock().await.ctx.operators.clone()
    }
```

One test: after a satisfied gate, `merge_session("m")` then
`InMemoryWorkspace::merged_message() == Some("m")`.

- [ ] **Step 5: Wire + run** — `lib.rs`: `pub mod git_workspace;` + `pub use git_workspace::GitWorkspace;`. Run `cargo test -p kontur-mcp` (unique temp dirs keep it parallel-safe), `cargo clippy -p kontur-mcp --all-targets -- -D warnings`.

- [ ] **Step 6: Commit** — targeted add of the five files; message: `feat(mcp): merge_session port + GitWorkspace with real git effects` (no trailer).

---

### Task 4: kontur-net — crate scaffold, protocol, codec

**Files:**
- Modify: `Cargo.toml` (workspace member)
- Create: `crates/kontur-net/Cargo.toml`, `src/lib.rs`, `src/protocol.rs`, `src/codec.rs`

**Interfaces:**
- Produces:

```rust
// protocol.rs (all Serialize + Deserialize + Clone + Debug + PartialEq)
pub enum ClientMsg {
    Hello { seat: String, operator: OperatorId },
    Ready,
    Cast { gate_id: GateId, verdict: CastVerdict },
    HandEdit { path: String, contents: String },
    Rotate,
    Bye,
}
pub enum ServerMsg { Welcome { seat: String }, State(WireState), Rejected { reason: String } }
pub struct WireSeat { pub label: String, pub operator: OperatorId, pub role: String, pub linked: bool, pub ready: bool }
pub enum WirePhase {
    AwaitOperators,
    DispatchReady { prompt: String },
    PlanReview { tasks: Vec<String> },
    Executing,
    Closed { gates: usize, chain_verified: bool, reviewers: Vec<String> },
}
pub struct WireFleetCard { pub id: String, pub status: String, pub tokens: u64, pub needs_signoff: bool }
pub struct WireGate {
    pub gate_id: GateId, pub task: String, pub files: Vec<String>, pub loc: u32,
    pub diff_hash: Hash, pub keys: Vec<VerdictView>, pub escalation_required: bool,
    pub diff_preview: Option<String>,
}
pub struct WireState {
    pub phase: WirePhase, pub seats: Vec<WireSeat>, pub fleet: Vec<WireFleetCard>,
    pub log: Vec<String>, pub gate: Option<WireGate>,
}
// codec.rs
pub async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, v: &T) -> io::Result<()>;
pub async fn read_json<R: AsyncBufRead + Unpin, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>>; // None on EOF
```

- [ ] **Step 1: Scaffold** — workspace member `crates/kontur-net`; Cargo.toml deps: `kontur-core` (path), `kontur-mcp` (path), `tokio` (features `rt-multi-thread, macros, sync, io-util, net, time`), `serde` (derive), `serde_json`, `thiserror`.

- [ ] **Step 2: protocol.rs** — the types above verbatim (import `kontur_core::{CastVerdict, GateId, Hash, OperatorId, VerdictView}`). Doc on `WireGate.keys`: sealing-safe by construction (`VerdictView`).

- [ ] **Step 3: codec.rs** — JSON lines:

```rust
pub async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, v: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await
}

pub async fn read_json<R: AsyncBufRead + Unpin, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>> {
    let mut line = String::new();
    if r.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

- [ ] **Step 4: Tests** — in-module: (a) round-trip every `ClientMsg`/`ServerMsg` variant through a `tokio::io::duplex` pair; (b) the sealed-wire test:

```rust
    #[test]
    fn sealed_key_on_the_wire_carries_no_value() {
        let view = VerdictView { operator: OperatorId([1; 32]), status: VerdictStatus::Sealed };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("Sealed"));
        assert!(!json.contains("Revealed"));
        assert!(!json.contains("\"Go\""));
        assert!(!json.contains("NoGo"));
    }
```

- [ ] **Step 5: Run + commit** — `cargo test -p kontur-net`; clippy clean. Commit: `feat(net): protocol types + JSON-lines codec` (targeted add incl. root Cargo.toml + Cargo.lock; no trailer).

---

### Task 5: kontur-net — SessionServer + ScriptedAgent

**Files:**
- Create: `crates/kontur-net/src/server.rs`, `crates/kontur-net/src/agent.rs`
- Modify: `crates/kontur-net/src/lib.rs`

**Interfaces:**
- Produces:

```rust
pub struct SessionConfig {
    pub prompt: String,
    pub plan: Vec<String>,                     // task descriptions shown at plan review
    pub seats: [(String, OperatorId); 2],      // (label, identity) for A and B
}
#[derive(Clone)]
pub struct SessionServer { /* Arc-shared inner; Clone is cheap and required by ScriptedAgent::run */ }
impl SessionServer {
    pub fn new(host: Arc<GateHost>, cfg: SessionConfig) -> Self;
    pub async fn attach<S: AsyncRead + AsyncWrite + Send + Unpin + 'static>(&self, stream: S); // spawns conn tasks
    pub fn state_rx(&self) -> tokio::sync::watch::Receiver<WireState>;      // for tests/agent
    pub fn plan_approved_rx(&self) -> tokio::sync::watch::Receiver<bool>;   // agent waits on this
    pub async fn agent_status(&self, card: WireFleetCard);                  // fleet panel updates
    pub async fn agent_log(&self, line: String);
    pub async fn agent_done(&self);            // triggers close when no gates pend
    pub fn host(&self) -> &Arc<GateHost>;
}
pub struct ScriptedAgent { pub tasks: Vec<ScriptedTask> }
pub struct ScriptedTask { pub id: String, pub path: String, pub contents: String }
impl ScriptedAgent {
    pub fn demo() -> Self; // two small tasks
    pub async fn run(self, server: SessionServer); // waits for plan approval, then executes
}
```

- [ ] **Step 1: server.rs.** Internal state:

```rust
enum Phase { AwaitOperators, DispatchReady, PlanReview, Executing, Closed }
struct SeatState { label: String, operator: OperatorId, role: String, linked: bool, ready: bool }
struct Net {
    phase: Phase, seats: [SeatState; 2], fleet: Vec<WireFleetCard>,
    log: std::collections::VecDeque<String>, agent_done: bool, started: std::time::Instant,
}
```

Core behaviours (each ends by calling `refresh().await`, which rebuilds `WireState`
from `Net` + `host.pending_gates()` (+ `gate_diff` for the first gate) and sends it
on the `watch`):

- `attach`: split the stream (`tokio::io::split`), `BufReader` the read half.
  First message must be `Hello { seat, operator }`; the operator must match one of
  `cfg.seats` — else `Rejected` + drop. Claim: set `linked=true`, send `Welcome`,
  then spawn (a) a writer task forwarding `watch` changes + a per-conn
  `mpsc::Receiver<ServerMsg>` (for `Rejected`), and (b) a reader loop dispatching
  messages. On reader EOF/error: `linked=false`, log
  `"<label> disconnected · gates park"`, refresh. Reconnect = new `attach` with the
  same operator.
- `Ready`: only meaningful in `DispatchReady`/`PlanReview`; set the seat's flag;
  when both true → advance phase (`DispatchReady`→`PlanReview` resetting flags;
  `PlanReview`→`Executing`, send `true` on the plan-approved watch), log the
  transition. Phase starts as `AwaitOperators`; when both seats are linked the
  first time → `DispatchReady`, log `"both stations linked"`.
- `Cast { gate_id, verdict }`: `host.submit_verdict`. `Err` → `Rejected { reason }`
  to that connection only. `Ok(progress)`: log `"<label> cast · sealed"`; if
  `Satisfied` log `"gate <id> · both keys in · accepted"`; if `Blocked` log
  `"gate <id> · no-go · remedy routed to agent"`.
- `HandEdit { path, contents }`: requires an active pending gate (else `Rejected`);
  `host.hand_edit(task_id_of_active_gate, &path, contents.as_bytes(), seat_operator)`;
  log `"<label> hand-edit <path> · applied · fresh gate"`.
- `Rotate`: swap the two seats' `role` strings, log.
- `agent_done` + `refresh`: when `Executing`, `agent_done`, and
  `host.pending_gates()` is empty → finalize: compose the merge message

```text
kontur session: <prompt-first-line>

Reviewed-by: <labelA> <hex16(opA)>
Reviewed-by: <labelB> <hex16(opB)>
```

  call `host.merge_session(&msg).await` (log failure rather than panic), compute
  `Closed { gates: host.audit_len().await, chain_verified: host.verify_audit().await.is_ok(), reviewers: [labelA, labelB] }`,
  set phase `Closed`, refresh. (`hex16` = first 16 hex chars of the operator key.)
- Log cap: keep the last 8 lines; timestamps as `mm:ss` since `started`.

- [ ] **Step 2: agent.rs.** `ScriptedAgent::run`: wait until `plan_approved_rx`
is true; for each task: `agent_status` (working card), `host.record_write`,
`begin_task_gate(task, tokens)` → await the returned `watch::Receiver<HoldState>`
to a terminal state (same borrow/`changed` loop as the mcp server handler); on
`Satisfied` → status card done, next task; on `Blocked` → read
`gate_outcome(gid)` remedy text, apply a "fix" write
(`contents + "\n// fix: " + steer + "\n"`), re-propose the same task (loop).
After all tasks: `agent_status` (idle), `agent_done().await`.
`ScriptedAgent::demo()`: two tasks (`t1` → `src/guard.rs`, `t2` → `src/tokens.rs`,
small contents).

- [ ] **Step 3: Tests** (in `server.rs` mod tests, over `tokio::io::duplex` with
raw `write_json`/`read_json` as the "clients"):
  - `two_operators_full_arc`: build `GateHost` (InMemoryWorkspace) + server +
    scripted agent (1 task). Connect two duplex clients, Hello both → state phase
    `DispatchReady`; both Ready → `PlanReview`; both Ready → `Executing`; await a
    state with `gate: Some(_)`; client A sends a signed `Cast` (sign with seed-1
    signer against the gate's `gate_id`/`diff_hash` from the state) → next state
    shows A's key `Sealed` (assert via the `WireGate.keys` `VerdictView`); B casts
    → eventually `Closed { chain_verified: true, .. }` and
    `InMemoryWorkspace::merged_message()` is `Some` containing both `Reviewed-by:` lines.
  - `nogo_routes_remedy_and_agent_reworks`: 1-task agent; A go, B no-go+steer →
    state returns to a fresh gate for the same task (the rework); both go →
    `Closed` with `gates == 2` (blocked + satisfied records) and chain verified.
  - `disconnect_parks_and_reconnect_resumes`: drop B's duplex mid-gate → state
    shows B `linked=false`; A's extra cast alone never closes the gate; re-attach
    B (new duplex, same operator) → cast → closes.
  - Use a wall-clock `Clock` impl for signing in tests (define a tiny
    `struct TestClock(i64)` implementing `kontur_core::Clock`, fixed values fine).

- [ ] **Step 4: Run + commit** — `cargo test -p kontur-net`; clippy clean.
Commit: `feat(net): SessionServer session arc + scripted agent` (no trailer).

---

### Task 6: kontur-net — SessionClient

**Files:**
- Create: `crates/kontur-net/src/client.rs`
- Modify: `crates/kontur-net/src/lib.rs`

**Interfaces:**
- Produces:

```rust
pub struct SystemClock;                          // impl kontur_core::Clock via SystemTime millis
pub struct SessionClient { /* writer + signer */ }
impl SessionClient {
    /// Handshake: sends Hello, awaits Welcome. Returns the client plus a stream
    /// of ServerMsg (states + rejections).
    pub async fn attach<S: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
        stream: S, seat: String, seed: [u8; 32],
    ) -> io::Result<(SessionClient, tokio::sync::mpsc::Receiver<ServerMsg>)>;
    pub async fn connect_tcp(addr: &str, seat: String, seed: [u8; 32])
        -> io::Result<(SessionClient, tokio::sync::mpsc::Receiver<ServerMsg>)>;
    pub fn operator(&self) -> OperatorId;
    pub async fn ready(&self) -> io::Result<()>;
    pub async fn rotate(&self) -> io::Result<()>;
    pub async fn hand_edit(&self, path: &str, contents: &str) -> io::Result<()>;
    /// Sign a go against the gate in the given wire state and send it.
    pub async fn cast_go(&self, gate: &WireGate) -> io::Result<()>;
    /// Sign a no-go with a steer remedy and send it.
    pub async fn cast_nogo(&self, gate: &WireGate, steer: &str) -> io::Result<()>;
}
```

`cast_go`/`cast_nogo` build `CastVerdict::create(&self.signer, &SystemClock, &gate.gate_id, gate.diff_hash, ..., ReviewDepth::FullDiff, None)` — the private key never leaves the client.

- [ ] **Step 1: Implement** (writer behind a `tokio::sync::Mutex`; a spawned reader task forwards every `ServerMsg` into the mpsc).

- [ ] **Step 2: Loopback test** — real `SessionServer` + two `SessionClient`s over duplex; drive the full happy arc (ready → plan → gate → A `cast_go` → assert the *other* client's next state shows A `Sealed` → B `cast_go` → `Closed` with `chain_verified`). Assert a `Rejected` arrives on a duplicate cast.

- [ ] **Step 3: Run + commit** — tests + clippy. Commit: `feat(net): SessionClient with client-side signing` (no trailer).

---

### Task 7: kontur-tui — remote mode + wired interactions

**Files:**
- Modify: `crates/kontur-tui/Cargo.toml` (add `kontur-net` path dep)
- Modify: `crates/kontur-tui/src/view.rs` (Prompt/Plan regions)
- Modify: `crates/kontur-tui/src/render.rs` (new arms, diff pane, close copy)
- Modify: `crates/kontur-tui/src/input.rs` (Ready action; hand-edit compose states already generic via remedy mode — add a `Compose` re-use, see below)
- Create: `crates/kontur-tui/src/remote.rs`
- Modify: `crates/kontur-tui/src/lib.rs`
- Modify: `crates/kontur-tui/tests/render.rs` (updated close copy + new golden tests)

**Interfaces:**
- Produces: `ActiveRegion::Prompt { prompt: String, ready: [bool; 2] }`, `ActiveRegion::Plan { tasks: Vec<String>, ready: [bool; 2] }`; `Action::Ready` (key `y`); `remote::wire_to_view(state: &WireState, own: OperatorId) -> SessionView`; `remote::run_remote(addr: &str, seat: String, seed: [u8; 32]) -> io::Result<()>`.

- [ ] **Step 1: view.rs** — add the two variants (fields above). `AuditSummary` unchanged.

- [ ] **Step 2: render.rs** —
  - `ActiveRegion::Prompt`: bordered `PROMPT` block: the prompt text, then
    `DISPATCH GATE   A ⟨■|□⟩ ready   B ⟨■|□⟩ ready` and the key line
    `[y] mark ready — needs both`.
  - `ActiveRegion::Plan`: bordered `PLAN` block listing tasks (`t1 …` lines) and
    the same both-ready bar + `[y] approve plan — needs both`.
  - Close copy: change the summary line from `" {} gates · unanimous"` to
    `" {} gates · chain {}"`-style: first line `" {} gates"` and keep the
    `chain verified ✓` line as-is; update the existing golden test string
    (`"4 gates"` instead of `"4 gates · unanimous"`).
  - Add `pub fn render_diff(frame: &mut Frame, title: &str, text: &str)` — full
    `frame.area()` bordered Paragraph (wrapped) with the key hint
    `[o] close diff` in the title; used by the app loop when diff view is toggled.

- [ ] **Step 3: input.rs** — add `Action::Ready` mapped from `KeyCode::Char('y')`
(non-composing mode only). Extend the composing mode unchanged (it is generic text
capture already; the remote loop tracks *what* is being composed).

- [ ] **Step 4: remote.rs** —
  - `wire_to_view`: seats → `Station { label, role: parse "DRIVER"/"NAVIGATOR", activity: linked ? "linked" : "dropped", operator }`; `StatusStrip { linked: both linked, four_eyes: true, fleet_count, needs_you, tokens: fleet sum }` where `needs_you` counts pending gates whose `keys` lack an entry for `own` (i.e. this seat's key is still awaited); fleet/log map directly (log lines into `LogLine { time: "", who: "", text }` — the server pre-formats); `phase` → `ActiveRegion` (`DispatchReady`→`Prompt`, `PlanReview`→`Plan`, `Executing` with `gate`→`Gate` (map `WireGate`→`GateCard`, keys via the same `key_for`-style match on `VerdictView` per seat), `Executing` without gate→`Idle`, `Closed`→`SessionClosed`).
  - `run_remote`: connect (`SessionClient::connect_tcp`), spawn a task folding the
    `ServerMsg` mpsc into a `watch<WireState>` (+ surface `Rejected.reason` into a
    transient status string shown on the command line for a few frames);
    `TerminalGuard::enter`; loop: build view (`wire_to_view`), render (or
    `render_diff` when toggled and the active gate has a preview); poll:
    `Ready`→`client.ready()`, `Go`→`cast_go(active gate)`, `NoGoBegin`→enter
    remedy compose→`RemedySubmit`→`cast_nogo(gate, &text)`, `HandEdit`→compose
    path then contents (two-stage compose; reuse the compose keys, track stage in
    the loop)→`client.hand_edit`, `OpenDiff`→toggle, `RotateRole`→`client.rotate()`,
    `Quit`→break. Restore terminal.
  - Compose-state struct local to `remote.rs`:

```rust
enum ComposeTarget { None, Remedy, HandEditPath, HandEditContents { path: String } }
```

- [ ] **Step 5: Tests** —
  - `remote.rs` unit tests for `wire_to_view`: sealed key stays `Sealed`;
    `needs_you` = 1 when own key absent, 0 once own key present (sealed);
    `DispatchReady` → `Prompt` with correct ready flags; `Closed` maps gates/
    verified/reviewers; `linked=false` on a seat → `StatusStrip.linked == false`.
  - Golden render tests: Prompt region (`DISPATCH GATE`, `[y] mark ready`),
    Plan region, dropped-link status (`B-STATION DROPPED`), diff pane
    (`render_diff` output contains the diff text + `[o] close diff`), updated
    close copy.

- [ ] **Step 6: Run + commit** — `cargo test -p kontur-tui`; clippy clean.
Commit: `feat(tui): remote two-seat mode + wired no-go/diff/hand-edit/ready` (no trailer).

---

### Task 8: kontur binary, agent endpoint, end-to-end test, docs

**Files:**
- Modify: `crates/kontur-tui/src/bin/kontur.rs`
- Modify: `crates/kontur-tui/Cargo.toml` (if needed)
- Create: `crates/kontur-net/tests/e2e.rs`
- Modify: `crates/kontur-net/src/lib.rs` (agent endpoint helper)
- Modify: `CLAUDE.md`, `README.md`

**Interfaces:**
- Produces: `kontur host|join|demo` CLI; `kontur_net::serve_agent_endpoint(listener: TcpListener, host: Arc<GateHost>)` (each accepted connection served by `kontur_mcp::KonturServer` via rmcp `serve_server` over the TCP stream).

- [ ] **Step 1: agent endpoint** — in `kontur-net` (new `src/agent_endpoint.rs`):

```rust
pub async fn serve_agent_endpoint(listener: tokio::net::TcpListener, host: Arc<GateHost>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else { break };
        let server = kontur_mcp::KonturServer::new(host.clone());
        tokio::spawn(async move {
            if let Ok(running) = rmcp::serve_server(server, stream).await {
                let _ = running.waiting().await;
            }
        });
    }
}
```

(Add `rmcp = { version = "2.2", features = ["server", "transport-async-rw"] }` to
`kontur-net`; reconcile the exact `serve_server`/`waiting` call against the
installed rmcp 2.2 source — the same pattern already used in
`crates/kontur-mcp/tests/server_mcp.rs`.)

- [ ] **Step 2: bin/kontur.rs** — hand-rolled `std::env::args` parsing:

```text
kontur demo
kontur host --repo <path> [--mem] [--operator-port 7777] [--agent-port 7778]
            [--prompt "..."] [--demo-agent] [--seeds 1,2] [--session s1]
kontur join --addr host:port --seat A|B --seed <n>
```

`host`: signers from seeds → operator ids → `SessionContext` (prompt, author =
seat A, agent id `agent-01`, model `external`, operators) → workspace
(`GitWorkspace::create(repo, session)` or `InMemoryWorkspace` with `--mem`) →
`GateHost` → `SessionServer::new` + `SessionConfig { prompt, plan: demo plan or
["external agent tasks"], seats: [("A", opA), ("B", opB)] }` → bind operator
listener, spawn accept-loop calling `server.attach(stream)`; bind agent listener →
`serve_agent_endpoint`; `--demo-agent` → spawn `ScriptedAgent::demo().run(server)`.
Print the join lines for each seat, then park on ctrl-c (`tokio::signal::ctrl_c`).
`join`: `run_remote(addr, seat, seed_bytes)` where `--seed n` → `[n as u8; 32]`.
`demo`: the existing local demo.

- [ ] **Step 3: e2e test** — `crates/kontur-net/tests/e2e.rs`: temp git repo (same
helper shape as GitWorkspace's tests) → `GitWorkspace` + `GateHost` +
`SessionServer` on `TcpListener::bind("127.0.0.1:0")` + accept-loop task +
`ScriptedAgent` (1 task) → two `SessionClient::connect_tcp` → ready ×2, plan ×2 →
await gate state → A `cast_go` → assert B's view of A is `Sealed` → B `cast_go` →
await `Closed { chain_verified: true, .. }` → assert the temp repo's `main` gained
exactly one commit whose message contains both `Reviewed-by:` lines.

- [ ] **Step 4: docs** —
  - `CLAUDE.md`: replace the "Stack & tooling" section ("Not yet chosen…") with the
    real stack (Rust workspace: kontur-core / kontur-mcp / kontur-net / kontur-tui;
    key deps rmcp, ratatui, tokio) and fill "Build / run / test":
    `cargo build` / `cargo test` / `cargo clippy --all-targets -- -D warnings` /
    `cargo run -p kontur-tui --bin kontur -- demo|host|join`.
  - `README.md`: replace the "Status: Concept / pre-build" section with a short
    **Running it** section (the three subcommands + the real-agent note: MCP over
    TCP at the agent port; Claude Code attaches via a stdio bridge, e.g.
    `{"command": "nc", "args": ["localhost", "7778"]}`; forcing native tools
    through the endpoint remains the CC-binding work).

- [ ] **Step 5: Run everything** — `cargo test` (whole workspace, twice for
parallel stability), `cargo clippy --all-targets -- -D warnings`,
`cargo build -p kontur-tui --bin kontur`.

- [ ] **Step 6: Commit** — targeted add; message: `feat: kontur host/join/demo binary, agent endpoint, e2e session test, docs` (no trailer).

---

## Self-review notes (for the executor)

- Spec coverage: §2 core → T1; blocked audit → T2; GitWorkspace/merge → T3; protocol → T4; server/agent/park/rotate → T5; client signing → T6; TUI remote + wired r/o/e/y + honesty fixes → T7; binary/endpoint/e2e/docs → T8. Invariant table §3 exercised across T1/T2 (6), T4-T6 (3, 7), T7 (4), T3+T8 (accept/merge reachability).
- Known adaptation points: rmcp `serve_server` call in T8 Step 1 (pattern exists in `kontur-mcp/tests/server_mcp.rs`); ratatui golden-cell details in T7 (pattern exists in `kontur-tui/tests/render.rs`).
- NO co-author trailers on any commit (global rule; earlier plans' templates are superseded).
- T5 is the largest task; if an implementer reports BLOCKED, split server core (state machine + refresh) from connection plumbing and re-dispatch.
