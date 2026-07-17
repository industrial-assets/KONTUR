# Kontur MCP Enforcement Plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `kontur-mcp`, a runnable MCP server that gates an agent's task-completion boundary through the four-eyes `kontur-core` engine and emits the tamper-evident audit record.

**Architecture:** A new async workspace crate. A `GateHost` owns session state behind a `tokio::sync::Mutex` (single writer, supplies `DualHold::cast`'s version guard), drives `kontur-core`'s dual-hold + audit chain, and calls a `Workspace` port for worktree side effects. An rmcp server exposes agent-facing tools; the gated `propose_task_complete` handler awaits its gate before returning. Operators drive an in-process operator face (verdicts, hand-edit) that the future net/TUI slice will implement.

**Tech Stack:** Rust (edition 2021), `kontur-core` (path dep), `rmcp` 2.2 (official MCP SDK, tokio-based), `tokio`, `serde`, `serde_json`, `thiserror`.

## Global Constraints

- Rust edition 2021, stable toolchain (1.93+ available).
- `kontur-core` stays synchronous/pure; async lives only in `kontur-mcp`.
- The `GateHost` is the **single writer** of session state and always passes `hold.version()` as `DualHold::cast`'s `expected_version` (no double-count; PRD §10.1 atomicity).
- **Blind sealing holds across the operator face:** `pending_gates()` / `GateView` must never expose a sealed verdict's value — project `kontur-core`'s `observed_verdicts()` (which returns `VerdictView`, already sealing-safe).
- **No single-key acceptance:** `Workspace::accept_task` is reachable only from a `SATISFIED` hold (invariants #1/#7). Operator loss ⇒ park (the handler keeps awaiting), never degrade.
- Operators sign verdicts client-side; the host stores only public `OperatorId`s — **never a private key, never log a key or a sealed verdict** (security is load-bearing).
- The audit chain is append-only; never mutate an emitted record (invariant #6).
- No `HashMap`/`HashSet` in any type fed to `canonical_bytes` (provenance and diffs use ordered `Vec`/structs). Runtime-only state (not serialized) may use maps, but this plan uses `Vec` for the hold registry so `pending_gates` order is deterministic.
- `cargo clippy --all-targets -- -D warnings` must be clean; test output pristine.
- `.gitignore` already excludes `/target`; **stage only the files each task changed** — never `git add -A`/`git add .`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## `kontur-core` public API this crate consumes (already built, reviewed)

`DualHold::{new, reopen_handedit, cast, state, version, outcome, observed_verdicts, gate_id, task_id, diff_hash, authorship, blocking_remedy}`, `HoldState`, `HoldOutcome`, `CastRejected`; `GatePolicy` (+`Independence, Availability, Authorship, Outcome`), `MakerSet`; `CastVerdict::{create, verify_signature}`, `Verdict`, `Remedy`, `ReviewDepth`, `VerdictView`; `Provenance`, `GateRecord::build`, `AuditChain`, `verify_chain`, `reviewed_by`, `GENESIS`, `ChainBreak`; `Signer`, `Clock`, `Ed25519Signer`, `FixedClock`, `sha256`, `OperatorId`, `GateId`, `TaskId`, `Hash`. (`GateRecord.core.gate_id` is a public field.)

---

## File structure

```
crates/kontur-mcp/
  Cargo.toml
  src/
    lib.rs           # module wiring + re-exports
    error.rs         # WorkspaceError, GateHostError
    session.rs       # SessionContext
    workspace.rs     # Workspace trait, FrozenDiff, CommandOutput, diff_hash(), InMemoryWorkspace
    provenance.rs    # build_provenance()
    gatehost.rs      # GateHost, SessionState, GateProgress/GateView/GateFinal, all orchestration
    fs_workspace.rs  # FsWorkspace (filesystem-backed)
    server.rs        # rmcp server: write_file / run_command / propose_task_complete
  tests/
    server_mcp.rs    # in-process rmcp client <-> server end-to-end
```

Gate-host orchestration tests live inline in `gatehost.rs` (they need crate-internal constructors); the MCP end-to-end test is an external integration test.

---

### Task 1: Crate scaffold + error types + session context

**Files:**
- Modify: `Cargo.toml` (workspace root — add member)
- Create: `crates/kontur-mcp/Cargo.toml`
- Create: `crates/kontur-mcp/src/lib.rs`
- Create: `crates/kontur-mcp/src/error.rs`
- Create: `crates/kontur-mcp/src/session.rs`

**Interfaces:**
- Produces: `WorkspaceError`, `GateHostError` (with `From<CastRejected>` and `From<WorkspaceError>`); `SessionContext { prompt, prompt_author, agent_id, agent_model, agent_version, operators: Vec<OperatorId>, policy: GatePolicy }` with `new(...)` (defaulting `policy` to `GatePolicy::default()`) and `with_policy`.

- [ ] **Step 1: Add the crate to the workspace** — edit the root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/kontur-core", "crates/kontur-mcp"]
```

- [ ] **Step 2: Create `crates/kontur-mcp/Cargo.toml`**

```toml
[package]
name = "kontur-mcp"
version = "0.1.0"
edition = "2021"

[dependencies]
kontur-core = { path = "../kontur-core" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "io-util"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
```

- [ ] **Step 3: Create `crates/kontur-mcp/src/lib.rs`**

```rust
//! Kontur MCP enforcement plane: gates an agent's task-completion boundary
//! through the four-eyes `kontur-core` engine and emits the audit record.

pub mod error;
pub mod session;

pub use error::{GateHostError, WorkspaceError};
pub use session::SessionContext;
```

- [ ] **Step 4: Create `crates/kontur-mcp/src/error.rs`**

```rust
use kontur_core::CastRejected;
use thiserror::Error;

/// Failures from the workspace port.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace io error: {0}")]
    Io(String),
    #[error("unknown task: {0}")]
    UnknownTask(String),
}

/// Failures from the gate host's operator/agent faces.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum GateHostError {
    #[error("unknown gate: {0}")]
    UnknownGate(String),
    #[error("verdict rejected: {0}")]
    Cast(#[from] CastRejected),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
}
```

- [ ] **Step 5: Create `crates/kontur-mcp/src/session.rs`**

```rust
use kontur_core::{GatePolicy, OperatorId};

/// Session-wide inputs shared across every gate: the co-constructed prompt, the
/// agent identity, the operator roster, and the gate policy.
#[derive(Clone, Debug)]
pub struct SessionContext {
    pub prompt: String,
    pub prompt_author: OperatorId,
    pub agent_id: String,
    pub agent_model: String,
    pub agent_version: String,
    /// Public identities of the operators supervising this session.
    pub operators: Vec<OperatorId>,
    pub policy: GatePolicy,
}

impl SessionContext {
    pub fn new(
        prompt: impl Into<String>,
        prompt_author: OperatorId,
        agent_id: impl Into<String>,
        agent_model: impl Into<String>,
        agent_version: impl Into<String>,
        operators: Vec<OperatorId>,
    ) -> Self {
        SessionContext {
            prompt: prompt.into(),
            prompt_author,
            agent_id: agent_id.into(),
            agent_model: agent_model.into(),
            agent_version: agent_version.into(),
            operators,
            policy: GatePolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: GatePolicy) -> Self {
        self.policy = policy;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kontur_core::{Independence, OperatorId};

    fn op(n: u8) -> OperatorId {
        OperatorId([n; 32])
    }

    #[test]
    fn defaults_to_core_gate_policy() {
        let ctx = SessionContext::new("do the thing", op(1), "agent-01", "claude", "1", vec![op(1), op(2)]);
        assert_eq!(ctx.policy, GatePolicy::default());
        assert_eq!(ctx.operators.len(), 2);
    }

    #[test]
    fn with_policy_overrides() {
        let p = GatePolicy { independence: Independence::Pragmatic, ..GatePolicy::default() };
        let ctx = SessionContext::new("x", op(1), "a", "m", "v", vec![op(1)]).with_policy(p);
        assert_eq!(ctx.policy.independence, Independence::Pragmatic);
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p kontur-mcp`
Expected: PASS (2 tests). Confirms the crate compiles and links `kontur-core`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/kontur-mcp/Cargo.toml crates/kontur-mcp/src/lib.rs crates/kontur-mcp/src/error.rs crates/kontur-mcp/src/session.rs
git commit -m "feat(mcp): scaffold kontur-mcp crate + error/session types

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Workspace port + InMemoryWorkspace

**Files:**
- Create: `crates/kontur-mcp/src/workspace.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`

**Interfaces:**
- Consumes: `WorkspaceError` (Task 1); `Hash`, `TaskId`, `sha256` (kontur-core).
- Produces:
  - `FrozenDiff { bytes: Vec<u8>, files: Vec<String>, loc: u32 }`, `CommandOutput { stdout: String, exit_code: i32 }`.
  - `trait Workspace: Send + Sync` with `apply_write`, `run_command`, `freeze_task_diff`, `accept_task`, `discard_task`.
  - `fn diff_hash(frozen: &FrozenDiff) -> Hash`.
  - `InMemoryWorkspace` with `new()`, and test-inspection methods `accepted_tasks()`, `discarded_tasks()`, `file_contents(task_id, path)`.

- [ ] **Step 1: Create `crates/kontur-mcp/src/workspace.rs`**

```rust
use std::sync::Mutex;

use kontur_core::{sha256, Hash, TaskId};

use crate::error::WorkspaceError;

/// A frozen snapshot of a task's pending changes, ready to hash and review.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FrozenDiff {
    /// Canonical byte representation of the diff (what gets hashed).
    pub bytes: Vec<u8>,
    pub files: Vec<String>,
    pub loc: u32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub exit_code: i32,
}

/// The worktree side-effect port. The gate host owns an `Arc<dyn Workspace>`.
/// Implementations must be cheap to call under the session lock (sync, fast).
pub trait Workspace: Send + Sync {
    fn apply_write(&self, task_id: &TaskId, path: &str, contents: &[u8]) -> Result<(), WorkspaceError>;
    fn run_command(&self, task_id: &TaskId, command: &str, cwd: &str) -> Result<CommandOutput, WorkspaceError>;
    fn freeze_task_diff(&self, task_id: &TaskId) -> Result<FrozenDiff, WorkspaceError>;
    fn accept_task(&self, task_id: &TaskId) -> Result<(), WorkspaceError>;
    fn discard_task(&self, task_id: &TaskId) -> Result<(), WorkspaceError>;
}

/// The single source of a diff's hash — used at open, sign, and record time so
/// the verdict signatures bind to exactly this diff.
pub fn diff_hash(frozen: &FrozenDiff) -> Hash {
    sha256(&frozen.bytes)
}

#[derive(Default)]
struct TaskBuf {
    writes: Vec<(String, Vec<u8>)>,
    commands: Vec<String>,
}

#[derive(Default)]
struct Inner {
    tasks: Vec<(String, TaskBuf)>, // Vec, not HashMap: deterministic order
    accepted: Vec<String>,
    discarded: Vec<String>,
}

impl Inner {
    fn task_mut(&mut self, id: &str) -> &mut TaskBuf {
        if !self.tasks.iter().any(|(t, _)| t == id) {
            self.tasks.push((id.to_string(), TaskBuf::default()));
        }
        &mut self.tasks.iter_mut().find(|(t, _)| t == id).unwrap().1
    }

    fn task(&self, id: &str) -> Option<&TaskBuf> {
        self.tasks.iter().find(|(t, _)| t == id).map(|(_, b)| b)
    }
}

/// In-memory workspace double: records writes/commands per task and reports a
/// deterministic frozen diff. Used by all orchestration tests.
pub struct InMemoryWorkspace {
    inner: Mutex<Inner>,
}

impl InMemoryWorkspace {
    pub fn new() -> Self {
        InMemoryWorkspace { inner: Mutex::new(Inner::default()) }
    }

    pub fn accepted_tasks(&self) -> Vec<TaskId> {
        self.inner.lock().unwrap().accepted.iter().cloned().map(TaskId).collect()
    }

    pub fn discarded_tasks(&self) -> Vec<TaskId> {
        self.inner.lock().unwrap().discarded.iter().cloned().map(TaskId).collect()
    }

    pub fn file_contents(&self, task_id: &TaskId, path: &str) -> Option<Vec<u8>> {
        let g = self.inner.lock().unwrap();
        g.task(&task_id.0)?
            .writes
            .iter()
            .rev()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.clone())
    }
}

impl Default for InMemoryWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace for InMemoryWorkspace {
    fn apply_write(&self, task_id: &TaskId, path: &str, contents: &[u8]) -> Result<(), WorkspaceError> {
        self.inner.lock().unwrap().task_mut(&task_id.0).writes.push((path.to_string(), contents.to_vec()));
        Ok(())
    }

    fn run_command(&self, task_id: &TaskId, command: &str, _cwd: &str) -> Result<CommandOutput, WorkspaceError> {
        self.inner.lock().unwrap().task_mut(&task_id.0).commands.push(command.to_string());
        Ok(CommandOutput { stdout: String::new(), exit_code: 0 })
    }

    fn freeze_task_diff(&self, task_id: &TaskId) -> Result<FrozenDiff, WorkspaceError> {
        let g = self.inner.lock().unwrap();
        let buf = g.task(&task_id.0).ok_or_else(|| WorkspaceError::UnknownTask(task_id.0.clone()))?;
        let mut bytes = Vec::new();
        let mut files = Vec::new();
        let mut loc = 0u32;
        for (path, contents) in &buf.writes {
            bytes.extend_from_slice(path.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(contents);
            bytes.push(b'\n');
            if !files.contains(path) {
                files.push(path.clone());
            }
            loc += contents.iter().filter(|b| **b == b'\n').count() as u32;
        }
        Ok(FrozenDiff { bytes, files, loc })
    }

    fn accept_task(&self, task_id: &TaskId) -> Result<(), WorkspaceError> {
        self.inner.lock().unwrap().accepted.push(task_id.0.clone());
        Ok(())
    }

    fn discard_task(&self, task_id: &TaskId) -> Result<(), WorkspaceError> {
        self.inner.lock().unwrap().discarded.push(task_id.0.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid() -> TaskId {
        TaskId("t1".into())
    }

    #[test]
    fn records_writes_and_freezes_deterministically() {
        let ws = InMemoryWorkspace::new();
        ws.apply_write(&tid(), "a.rs", b"line1\nline2\n").unwrap();
        ws.apply_write(&tid(), "b.rs", b"x\n").unwrap();
        let f1 = ws.freeze_task_diff(&tid()).unwrap();
        let f2 = ws.freeze_task_diff(&tid()).unwrap();
        assert_eq!(f1, f2);
        assert_eq!(f1.files, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(f1.loc, 3);
        assert_eq!(diff_hash(&f1), diff_hash(&f2));
    }

    #[test]
    fn accept_and_discard_are_observable() {
        let ws = InMemoryWorkspace::new();
        ws.apply_write(&tid(), "a.rs", b"x").unwrap();
        ws.accept_task(&tid()).unwrap();
        assert_eq!(ws.accepted_tasks(), vec![tid()]);
        assert!(ws.discarded_tasks().is_empty());
        assert_eq!(ws.file_contents(&tid(), "a.rs"), Some(b"x".to_vec()));
    }

    #[test]
    fn freeze_unknown_task_errors() {
        let ws = InMemoryWorkspace::new();
        assert_eq!(
            ws.freeze_task_diff(&TaskId("nope".into())).unwrap_err(),
            WorkspaceError::UnknownTask("nope".into())
        );
    }
}
```

- [ ] **Step 2: Wire the module** — edit `crates/kontur-mcp/src/lib.rs`:

```rust
pub mod error;
pub mod session;
pub mod workspace;

pub use error::{GateHostError, WorkspaceError};
pub use session::SessionContext;
pub use workspace::{diff_hash, CommandOutput, FrozenDiff, InMemoryWorkspace, Workspace};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-mcp workspace`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-mcp/src/workspace.rs crates/kontur-mcp/src/lib.rs
git commit -m "feat(mcp): Workspace port + InMemoryWorkspace double

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Provenance assembly

**Files:**
- Create: `crates/kontur-mcp/src/provenance.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`

**Interfaces:**
- Consumes: `SessionContext` (Task 1), `FrozenDiff` (Task 2), `Provenance`, `Hash`, `TaskId` (kontur-core).
- Produces: `fn build_provenance(ctx: &SessionContext, task_id: &TaskId, diff_hash: Hash, frozen: &FrozenDiff, tokens: u64) -> Provenance`.

- [ ] **Step 1: Create `crates/kontur-mcp/src/provenance.rs`**

```rust
use kontur_core::{Hash, Provenance, TaskId};

use crate::session::SessionContext;
use crate::workspace::FrozenDiff;

/// Assemble a `kontur_core::Provenance` for a gate from the session context and
/// the frozen task diff. Note: `Provenance` has no tool-trail field — the
/// tool-trail is recorded on the workspace, not folded into the signed record.
pub fn build_provenance(
    ctx: &SessionContext,
    task_id: &TaskId,
    diff_hash: Hash,
    frozen: &FrozenDiff,
    tokens: u64,
) -> Provenance {
    Provenance {
        task_id: task_id.clone(),
        prompt: ctx.prompt.clone(),
        prompt_author: ctx.prompt_author,
        agent_id: ctx.agent_id.clone(),
        agent_model: ctx.agent_model.clone(),
        agent_version: ctx.agent_version.clone(),
        diff_hash,
        files: frozen.files.clone(),
        loc: frozen.loc,
        tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{diff_hash, InMemoryWorkspace, Workspace};
    use kontur_core::OperatorId;

    #[test]
    fn maps_session_and_diff_fields() {
        let ws = InMemoryWorkspace::new();
        let task = TaskId("t1".into());
        ws.apply_write(&task, "a.rs", b"x\n").unwrap();
        let frozen = ws.freeze_task_diff(&task).unwrap();
        let dh = diff_hash(&frozen);

        let ctx = SessionContext::new("do it", OperatorId([1; 32]), "agent-01", "claude", "1.0", vec![OperatorId([1; 32])]);
        let p = build_provenance(&ctx, &task, dh, &frozen, 6400);

        assert_eq!(p.task_id, task);
        assert_eq!(p.prompt, "do it");
        assert_eq!(p.agent_id, "agent-01");
        assert_eq!(p.diff_hash, dh);
        assert_eq!(p.files, vec!["a.rs".to_string()]);
        assert_eq!(p.loc, 1);
        assert_eq!(p.tokens, 6400);
    }
}
```

- [ ] **Step 2: Wire the module** — edit `crates/kontur-mcp/src/lib.rs`, add `pub mod provenance;` and `pub use provenance::build_provenance;`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-mcp provenance`
Expected: PASS (1 test).

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-mcp/src/provenance.rs crates/kontur-mcp/src/lib.rs
git commit -m "feat(mcp): provenance assembly from session + frozen diff

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: GateHost — open a gate + satisfied path

**Files:**
- Create: `crates/kontur-mcp/src/gatehost.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–3; `DualHold`, `HoldState`, `MakerSet`, `Authorship`, `AuditChain`, `GateRecord`, `verify_chain`, `reviewed_by`, `ChainBreak`, `GateId`, `TaskId`, `CastVerdict`, `Provenance`, `OperatorId` (kontur-core); `tokio::sync::{Mutex, watch}`.
- Produces:
  - `GateProgress { state: HoldState, escalation_required: bool, remedy: Option<Remedy> }`.
  - `GateHost` with `new(ctx, workspace: Arc<dyn Workspace>)`, `open_gate(task_id, provenance) -> (GateId, watch::Receiver<HoldState>)`, `submit_verdict(&GateId, CastVerdict) -> Result<GateProgress, GateHostError>`, `record_write`, `run_command`, `verify_audit() -> Result<(), ChainBreak>`, `reviewed_by(&GateId) -> Option<Vec<OperatorId>>`.

- [ ] **Step 1: Create `crates/kontur-mcp/src/gatehost.rs`**

```rust
use std::sync::Arc;

use kontur_core::{
    reviewed_by as core_reviewed_by, verify_chain, Authorship, AuditChain, CastVerdict, ChainBreak,
    DualHold, GateId, GateRecord, HoldState, MakerSet, OperatorId, Provenance, Remedy, TaskId,
};
use tokio::sync::{watch, Mutex};

use crate::error::GateHostError;
use crate::session::SessionContext;
use crate::workspace::{CommandOutput, Workspace};

/// Result of a cast on the operator face.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GateProgress {
    pub state: HoldState,
    pub escalation_required: bool,
    /// Present only when the gate is `Blocked` — the remedy driving rework.
    pub remedy: Option<Remedy>,
}

struct HoldEntry {
    hold: DualHold,
    provenance: Provenance,
    watch_tx: watch::Sender<HoldState>,
    escalation_required: bool,
}

struct SessionState {
    ctx: SessionContext,
    chain: AuditChain,
    holds: Vec<HoldEntry>,
    next_gate: u64,
}

/// Owns session state behind a single lock and drives `kontur-core`.
pub struct GateHost {
    state: Arc<Mutex<SessionState>>,
    workspace: Arc<dyn Workspace>,
}

impl GateHost {
    pub fn new(ctx: SessionContext, workspace: Arc<dyn Workspace>) -> Self {
        GateHost {
            state: Arc::new(Mutex::new(SessionState {
                ctx,
                chain: AuditChain::new(),
                holds: Vec::new(),
                next_gate: 0,
            })),
            workspace,
        }
    }

    /// Agent face: record a worktree write on a task (not gated).
    pub async fn record_write(&self, task_id: &TaskId, path: &str, contents: &[u8]) -> Result<(), GateHostError> {
        self.workspace.apply_write(task_id, path, contents)?;
        Ok(())
    }

    /// Agent face: run a command in the worktree (not gated).
    pub async fn run_command(&self, task_id: &TaskId, command: &str, cwd: &str) -> Result<CommandOutput, GateHostError> {
        Ok(self.workspace.run_command(task_id, command, cwd)?)
    }

    /// Open a gate over a task's frozen diff. Returns the gate id and a receiver
    /// the awaiting agent-side handler watches for resolution.
    pub async fn open_gate(&self, task_id: TaskId, provenance: Provenance) -> (GateId, watch::Receiver<HoldState>) {
        let mut st = self.state.lock().await;
        st.next_gate += 1;
        let id = GateId(format!("gate-{:03}", st.next_gate));
        let hold = DualHold::new(
            id.clone(),
            task_id,
            provenance.diff_hash,
            st.ctx.policy,
            MakerSet::new(),
            Authorship::Agent,
        );
        let (tx, rx) = watch::channel(HoldState::Open);
        st.holds.push(HoldEntry { hold, provenance, watch_tx: tx, escalation_required: false });
        (id, rx)
    }

    /// Operator face: cast a signed verdict on a gate. On resolution, accepts or
    /// discards the task and publishes the new state on the gate's watch.
    pub async fn submit_verdict(&self, gate_id: &GateId, cv: CastVerdict) -> Result<GateProgress, GateHostError> {
        let mut st = self.state.lock().await;
        let idx = st
            .holds
            .iter()
            .position(|e| e.hold.gate_id() == gate_id)
            .ok_or_else(|| GateHostError::UnknownGate(gate_id.0.clone()))?;

        let ev = st.holds[idx].hold.version();
        let outcome = st.holds[idx].hold.cast(ev, cv)?;
        st.holds[idx].escalation_required = outcome.escalation_required;
        let state = outcome.state;

        let remedy = match state {
            HoldState::Satisfied => {
                let prev = st.chain.head();
                let (task_id, record) = {
                    let e = &st.holds[idx];
                    let rec = GateRecord::build(prev, e.provenance.clone(), &e.hold)
                        .expect("a satisfied hold always builds a record");
                    (e.hold.task_id().clone(), rec)
                };
                self.workspace.accept_task(&task_id)?;
                st.chain.append(record).expect("chain head matches prev by construction");
                None
            }
            HoldState::Blocked => {
                let (task_id, remedy) = {
                    let e = &st.holds[idx];
                    (e.hold.task_id().clone(), e.hold.blocking_remedy())
                };
                self.workspace.discard_task(&task_id)?;
                remedy
            }
            _ => None,
        };

        let _ = st.holds[idx].watch_tx.send(state);
        Ok(GateProgress { state, escalation_required: st.holds[idx].escalation_required, remedy })
    }

    /// Verify the whole audit chain (tamper-evidence check).
    pub async fn verify_audit(&self) -> Result<(), ChainBreak> {
        let st = self.state.lock().await;
        verify_chain(st.chain.records())
    }

    /// The operators whose verified go-signatures back a gate's record.
    pub async fn reviewed_by(&self, gate_id: &GateId) -> Option<Vec<OperatorId>> {
        let st = self.state.lock().await;
        st.chain
            .records()
            .iter()
            .find(|r| &r.core.gate_id == gate_id)
            .map(core_reviewed_by)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::build_provenance;
    use crate::workspace::{diff_hash, InMemoryWorkspace, Workspace};
    use kontur_core::{Ed25519Signer, FixedClock, Hash, ReviewDepth, Signer, Verdict};

    fn ctx(ops: Vec<OperatorId>) -> SessionContext {
        SessionContext::new("do the thing", ops[0], "agent-01", "claude", "1.0", ops)
    }

    fn go_verdict(seed: u8, gate_id: &GateId, dh: Hash) -> CastVerdict {
        let signer = Ed25519Signer::from_seed([seed; 32]);
        CastVerdict::create(&signer, &FixedClock(1000 + seed as i64), gate_id, dh, Verdict::Go, ReviewDepth::FullDiff, None)
    }

    async fn open_a_gate(host: &GateHost, ws: &InMemoryWorkspace, ctx: &SessionContext) -> (GateId, Hash) {
        let task = TaskId("t1".into());
        ws.apply_write(&task, "a.rs", b"x\n").unwrap();
        let frozen = ws.freeze_task_diff(&task).unwrap();
        let dh = diff_hash(&frozen);
        let prov = build_provenance(ctx, &task, dh, &frozen, 100);
        let (gid, _rx) = host.open_gate(task, prov).await;
        (gid, dh)
    }

    #[tokio::test]
    async fn two_go_verdicts_satisfy_and_accept() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());

        let (gid, dh) = open_a_gate(&host, &ws, &context).await;

        let p1 = host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        assert_eq!(p1.state, HoldState::Partial);
        assert!(ws.accepted_tasks().is_empty());

        let p2 = host.submit_verdict(&gid, go_verdict(2, &gid, dh)).await.unwrap();
        assert_eq!(p2.state, HoldState::Satisfied);

        assert_eq!(ws.accepted_tasks(), vec![TaskId("t1".into())]);
        assert!(host.verify_audit().await.is_ok());
        assert_eq!(host.reviewed_by(&gid).await.unwrap().len(), 2);
    }
}
```

- [ ] **Step 2: Wire the module** — edit `crates/kontur-mcp/src/lib.rs`, add `pub mod gatehost;` and `pub use gatehost::{GateHost, GateProgress};`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-mcp gatehost`
Expected: PASS (1 test). Then `cargo build -p kontur-mcp --all-targets 2>&1` — zero warnings (fix any unused import per the note above).

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-mcp/src/gatehost.rs crates/kontur-mcp/src/lib.rs
git commit -m "feat(mcp): GateHost open_gate + satisfied path with audit emission

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: GateHost — blocked path, pending gates, gate outcome, task-gate helper

**Files:**
- Modify: `crates/kontur-mcp/src/gatehost.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`

**Interfaces:**
- Consumes: `VerdictView`, `Hash`, `Remedy`, `FrozenDiff` (kontur-core / Task 2), `build_provenance` (Task 3), `diff_hash` (Task 2).
- Produces (added to `GateHost`):
  - `GateView { gate_id, task_id, diff_hash: Hash, state, observed: Vec<VerdictView>, escalation_required }`.
  - `GateFinal { state, remedy: Option<Remedy>, reviewed_by: Vec<OperatorId> }`.
  - `pending_gates() -> Vec<GateView>`, `gate_outcome(&GateId) -> Option<GateFinal>`, `begin_task_gate(TaskId, tokens: u64) -> Result<(GateId, watch::Receiver<HoldState>), GateHostError>`.

- [ ] **Step 1: Add the view/final structs and methods** — in `crates/kontur-mcp/src/gatehost.rs`.

Add to the imports (top of file): `VerdictView` and `Hash` from `kontur_core`, and `build_provenance`, `diff_hash` from the crate:

```rust
use kontur_core::{Hash, VerdictView};

use crate::provenance::build_provenance;
use crate::workspace::diff_hash;
```

Add these structs near `GateProgress`:

```rust
/// Operator-face projection of a pending gate. Never exposes a sealed verdict
/// value — `observed` is `kontur-core`'s sealing-safe `VerdictView`.
#[derive(Clone, Debug)]
pub struct GateView {
    pub gate_id: GateId,
    pub task_id: TaskId,
    pub diff_hash: Hash,
    pub state: HoldState,
    pub observed: Vec<VerdictView>,
    pub escalation_required: bool,
}

/// Terminal summary of a gate, read by the awaiting agent handler.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GateFinal {
    pub state: HoldState,
    pub remedy: Option<Remedy>,
    pub reviewed_by: Vec<OperatorId>,
}
```

Add these methods inside `impl GateHost` (before the closing brace):

```rust
    /// Agent face: freeze the task diff, build provenance, and open its gate.
    /// Composes the workspace + provenance so the server stays thin.
    pub async fn begin_task_gate(
        &self,
        task_id: TaskId,
        tokens: u64,
    ) -> Result<(GateId, watch::Receiver<HoldState>), GateHostError> {
        let frozen = self.workspace.freeze_task_diff(&task_id)?;
        let dh = diff_hash(&frozen);
        let provenance = {
            let st = self.state.lock().await;
            build_provenance(&st.ctx, &task_id, dh, &frozen, tokens)
        };
        Ok(self.open_gate(task_id, provenance).await)
    }

    /// Operator face: gates awaiting review, sealing-safe.
    pub async fn pending_gates(&self) -> Vec<GateView> {
        let st = self.state.lock().await;
        st.holds
            .iter()
            .filter(|e| matches!(e.hold.state(), HoldState::Open | HoldState::Partial))
            .map(|e| GateView {
                gate_id: e.hold.gate_id().clone(),
                task_id: e.hold.task_id().clone(),
                diff_hash: e.hold.diff_hash(),
                state: e.hold.state(),
                observed: e.hold.observed_verdicts(),
                escalation_required: e.escalation_required,
            })
            .collect()
    }

    /// Read a gate's terminal outcome (for the awaiting agent handler).
    pub async fn gate_outcome(&self, gate_id: &GateId) -> Option<GateFinal> {
        let st = self.state.lock().await;
        let e = st.holds.iter().find(|e| e.hold.gate_id() == gate_id)?;
        let state = e.hold.state();
        let remedy = e.hold.blocking_remedy();
        let reviewed_by = st
            .chain
            .records()
            .iter()
            .find(|r| &r.core.gate_id == gate_id)
            .map(core_reviewed_by)
            .unwrap_or_default();
        Some(GateFinal { state, remedy, reviewed_by })
    }
```

- [ ] **Step 2: Write the failing tests** — add to the `tests` module in `gatehost.rs`:

```rust
    fn nogo_verdict(seed: u8, gate_id: &GateId, dh: Hash, steer: &str) -> CastVerdict {
        let signer = Ed25519Signer::from_seed([seed; 32]);
        CastVerdict::create(
            &signer,
            &FixedClock(2000 + seed as i64),
            gate_id,
            dh,
            Verdict::NoGo(kontur_core::Remedy::Steer(steer.into())),
            ReviewDepth::FullDiff,
            None,
        )
    }

    #[tokio::test]
    async fn nogo_blocks_discards_and_returns_remedy() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());
        let (gid, dh) = open_a_gate(&host, &ws, &context).await;

        host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        let p2 = host.submit_verdict(&gid, nogo_verdict(2, &gid, dh, "cache it")).await.unwrap();

        assert_eq!(p2.state, HoldState::Blocked);
        assert_eq!(p2.remedy, Some(kontur_core::Remedy::Steer("cache it".into())));
        assert!(ws.accepted_tasks().is_empty());
        assert_eq!(ws.discarded_tasks(), vec![TaskId("t1".into())]);
    }

    #[tokio::test]
    async fn pending_gates_hides_sealed_first_verdict() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());
        let (gid, dh) = open_a_gate(&host, &ws, &context).await;

        host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        let pending = host.pending_gates().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].state, HoldState::Partial);
        assert_eq!(pending[0].observed.len(), 1);
        assert_eq!(pending[0].observed[0].status, kontur_core::VerdictStatus::Sealed);
        assert_eq!(pending[0].diff_hash, dh);
    }

    #[tokio::test]
    async fn begin_task_gate_and_outcome_reports_satisfied() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());

        let task = TaskId("t1".into());
        ws.apply_write(&task, "a.rs", b"x\n").unwrap();
        let (gid, _rx) = host.begin_task_gate(task, 42).await.unwrap();
        let dh = host.pending_gates().await[0].diff_hash;

        host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        host.submit_verdict(&gid, go_verdict(2, &gid, dh)).await.unwrap();

        let outcome = host.gate_outcome(&gid).await.unwrap();
        assert_eq!(outcome.state, HoldState::Satisfied);
        assert_eq!(outcome.reviewed_by.len(), 2);
        assert!(outcome.remedy.is_none());
    }
```

Add `use kontur_core::VerdictStatus;` is not needed — tests reference `kontur_core::VerdictStatus` fully-qualified.

- [ ] **Step 3: Wire re-exports** — edit `crates/kontur-mcp/src/lib.rs`, extend the gatehost re-export:

```rust
pub use gatehost::{GateFinal, GateHost, GateProgress, GateView};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p kontur-mcp gatehost`
Expected: PASS (4 tests). Then `cargo build -p kontur-mcp --all-targets 2>&1` — zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/kontur-mcp/src/gatehost.rs crates/kontur-mcp/src/lib.rs
git commit -m "feat(mcp): blocked path, pending gates, outcome, task-gate helper

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: GateHost — hand-edit + escalation

**Files:**
- Modify: `crates/kontur-mcp/src/gatehost.rs`

**Interfaces:**
- Consumes: `DualHold::reopen_handedit` (kontur-core).
- Produces (added to `GateHost`): `hand_edit(task_id: TaskId, path: &str, contents: &[u8], editor: OperatorId) -> Result<GateId, GateHostError>`.

- [ ] **Step 1: Add the method** — inside `impl GateHost` in `gatehost.rs`:

```rust
    /// Operator face: a hand-edit. Applies to the worktree immediately, then
    /// opens a FRESH gate over the combined diff (deferred acceptance). The
    /// editor joins the maker set (strict mode excludes them); escalation is
    /// signalled on the first cast when the eligible pool < required.
    pub async fn hand_edit(
        &self,
        task_id: TaskId,
        path: &str,
        contents: &[u8],
        editor: OperatorId,
    ) -> Result<GateId, GateHostError> {
        self.workspace.apply_write(&task_id, path, contents)?;
        let frozen = self.workspace.freeze_task_diff(&task_id)?;
        let dh = diff_hash(&frozen);

        let mut st = self.state.lock().await;
        st.next_gate += 1;
        let id = GateId(format!("gate-{:03}", st.next_gate));
        let provenance = build_provenance(&st.ctx, &task_id, dh, &frozen, 0);
        let hold = DualHold::reopen_handedit(
            id.clone(),
            task_id,
            dh,
            st.ctx.policy,
            MakerSet::new(),
            editor,
            true,
            &st.ctx.operators,
        );
        let (tx, _rx) = watch::channel(hold.state());
        st.holds.push(HoldEntry { hold, provenance, watch_tx: tx, escalation_required: false });
        Ok(id)
    }
```

- [ ] **Step 2: Write the failing tests** — add to the `tests` module in `gatehost.rs`:

```rust
    #[tokio::test]
    async fn hand_edit_applies_now_and_opens_fresh_gate() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let task = TaskId("t1".into());
        // Pragmatic policy so the editor (op1) may co-sign.
        let host = GateHost::new(
            ctx(vec![op1, op2]).with_policy(kontur_core::GatePolicy {
                independence: kontur_core::Independence::Pragmatic,
                ..kontur_core::GatePolicy::default()
            }),
            ws.clone(),
        );

        let gid = host.hand_edit(task.clone(), "a.rs", b"guarded\n", op1).await.unwrap();
        // Applied immediately, observable in the workspace.
        assert_eq!(ws.file_contents(&task, "a.rs"), Some(b"guarded\n".to_vec()));

        let dh = host.pending_gates().await[0].diff_hash;
        // Editor op1 co-signs (pragmatic), op2 co-signs -> satisfied.
        host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        let p = host.submit_verdict(&gid, go_verdict(2, &gid, dh)).await.unwrap();
        assert_eq!(p.state, HoldState::Satisfied);
        assert_eq!(ws.accepted_tasks(), vec![task]);
    }

    #[tokio::test]
    async fn hand_edit_strict_signals_escalation_and_excludes_editor() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        // Default policy = strict.
        let host = GateHost::new(ctx(vec![op1, op2]), ws.clone());

        let task = TaskId("t1".into());
        let gid = host.hand_edit(task, "a.rs", b"guarded\n", op1).await.unwrap();
        let dh = host.pending_gates().await[0].diff_hash;

        // op2 (non-editor) casts: accepted, but escalation is signalled (pool = 1 < 2).
        let p = host.submit_verdict(&gid, go_verdict(2, &gid, dh)).await.unwrap();
        assert!(p.escalation_required);

        // op1 (the editor) is a maker in strict mode -> rejected.
        let err = host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap_err();
        assert_eq!(err, GateHostError::Cast(kontur_core::CastRejected::Ineligible));
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-mcp gatehost`
Expected: PASS (6 tests). Then `cargo build -p kontur-mcp --all-targets 2>&1` — zero warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-mcp/src/gatehost.rs
git commit -m "feat(mcp): hand-edit through the gate host with escalation signalling

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: FsWorkspace (filesystem-backed)

**Files:**
- Create: `crates/kontur-mcp/src/fs_workspace.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`

**Interfaces:**
- Consumes: `Workspace`, `FrozenDiff`, `CommandOutput`, `WorkspaceError` (Task 2).
- Produces: `FsWorkspace` with `new(root: PathBuf) -> Self`, implementing `Workspace` against a real directory. Tracks per-task written paths for `freeze`/`discard`.

- [ ] **Step 1: Create `crates/kontur-mcp/src/fs_workspace.rs`**

```rust
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use kontur_core::TaskId;

use crate::error::WorkspaceError;
use crate::workspace::{CommandOutput, FrozenDiff, Workspace};

/// Filesystem-backed workspace: writes land under `root`, commands run via the
/// system shell. `accept_task` records acceptance (the real git commit/merge is
/// a later slice); `discard_task` removes the task's written files.
pub struct FsWorkspace {
    root: PathBuf,
    tracked: Mutex<Vec<(String, Vec<String>)>>, // (task_id, relative paths written)
}

impl FsWorkspace {
    pub fn new(root: PathBuf) -> Self {
        FsWorkspace { root, tracked: Mutex::new(Vec::new()) }
    }

    fn track(&self, task_id: &str, path: &str) {
        let mut g = self.tracked.lock().unwrap();
        if let Some((_, paths)) = g.iter_mut().find(|(t, _)| t == task_id) {
            if !paths.contains(&path.to_string()) {
                paths.push(path.to_string());
            }
        } else {
            g.push((task_id.to_string(), vec![path.to_string()]));
        }
    }

    fn paths_for(&self, task_id: &str) -> Vec<String> {
        self.tracked
            .lock()
            .unwrap()
            .iter()
            .find(|(t, _)| t == task_id)
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    }
}

impl Workspace for FsWorkspace {
    fn apply_write(&self, task_id: &TaskId, path: &str, contents: &[u8]) -> Result<(), WorkspaceError> {
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WorkspaceError::Io(e.to_string()))?;
        }
        std::fs::write(&full, contents).map_err(|e| WorkspaceError::Io(e.to_string()))?;
        self.track(&task_id.0, path);
        Ok(())
    }

    fn run_command(&self, _task_id: &TaskId, command: &str, cwd: &str) -> Result<CommandOutput, WorkspaceError> {
        let dir = if cwd.is_empty() { self.root.clone() } else { self.root.join(cwd) };
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&dir)
            .output()
            .map_err(|e| WorkspaceError::Io(e.to_string()))?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    fn freeze_task_diff(&self, task_id: &TaskId) -> Result<FrozenDiff, WorkspaceError> {
        let paths = self.paths_for(&task_id.0);
        if paths.is_empty() {
            return Err(WorkspaceError::UnknownTask(task_id.0.clone()));
        }
        let mut bytes = Vec::new();
        let mut loc = 0u32;
        for path in &paths {
            let contents = std::fs::read(self.root.join(path)).map_err(|e| WorkspaceError::Io(e.to_string()))?;
            bytes.extend_from_slice(path.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(&contents);
            bytes.push(b'\n');
            loc += contents.iter().filter(|b| **b == b'\n').count() as u32;
        }
        Ok(FrozenDiff { bytes, files: paths, loc })
    }

    fn accept_task(&self, _task_id: &TaskId) -> Result<(), WorkspaceError> {
        // Acceptance recorded; the real git commit/merge is a later slice.
        Ok(())
    }

    fn discard_task(&self, task_id: &TaskId) -> Result<(), WorkspaceError> {
        for path in self.paths_for(&task_id.0) {
            let _ = std::fs::remove_file(self.root.join(path));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::diff_hash;

    fn temp_root() -> PathBuf {
        // Deterministic-per-process unique dir without external crates.
        let mut p = std::env::temp_dir();
        p.push(format!("kontur-fsws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn writes_land_on_disk_and_freeze_is_stable() {
        let root = temp_root();
        let ws = FsWorkspace::new(root.clone());
        let task = TaskId("t1".into());
        ws.apply_write(&task, "src/a.txt", b"hello\n").unwrap();
        assert_eq!(std::fs::read(root.join("src/a.txt")).unwrap(), b"hello\n");
        let f1 = ws.freeze_task_diff(&task).unwrap();
        let f2 = ws.freeze_task_diff(&task).unwrap();
        assert_eq!(diff_hash(&f1), diff_hash(&f2));
        assert_eq!(f1.files, vec!["src/a.txt".to_string()]);
    }

    #[test]
    fn run_command_executes() {
        let ws = FsWorkspace::new(temp_root());
        let out = ws.run_command(&TaskId("t1".into()), "echo hi", "").unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("hi"));
    }

    #[test]
    fn discard_removes_written_files() {
        let root = temp_root();
        let ws = FsWorkspace::new(root.clone());
        let task = TaskId("t2".into());
        ws.apply_write(&task, "gone.txt", b"x").unwrap();
        assert!(root.join("gone.txt").exists());
        ws.discard_task(&task).unwrap();
        assert!(!root.join("gone.txt").exists());
    }
}
```

> **Note:** the three tests share a process-id-based temp dir prefix but write distinct filenames/tasks, so they don't collide. If a reviewer prefers stronger isolation, each test may append its own subdirectory — not required for correctness here.

- [ ] **Step 2: Wire the module** — edit `crates/kontur-mcp/src/lib.rs`, add `pub mod fs_workspace;` and `pub use fs_workspace::FsWorkspace;`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-mcp fs_workspace`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-mcp/src/fs_workspace.rs crates/kontur-mcp/src/lib.rs
git commit -m "feat(mcp): filesystem-backed FsWorkspace

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: rmcp server + in-process MCP end-to-end tests

**Files:**
- Modify: `crates/kontur-mcp/Cargo.toml` (add `rmcp`, `serde_json`)
- Create: `crates/kontur-mcp/src/server.rs`
- Modify: `crates/kontur-mcp/src/lib.rs`
- Create: `crates/kontur-mcp/tests/server_mcp.rs`

**Interfaces:**
- Consumes: `GateHost`, `HoldState`, `TaskId`, `Remedy` (prior tasks); rmcp 2.2 API.
- Produces: `KonturServer` (an rmcp tools-only `ServerHandler`) exposing `write_file`, `run_command`, `propose_task_complete`.

**GROUNDING — the installed rmcp source is the source of truth.** rmcp 2.2.0 is extracted at:
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-2.2.0/`
Reference these files if a signature differs from the code below (the SDK evolves):
- Tool macro pattern & `Parameters`/`Json`: `src/handler/server/router/tool.rs` (doc example at top).
- `serve_server` / client `().serve(transport)`: `src/service.rs` (doc at ~line 769), `src/service/server.rs`, `src/service/client.rs`.
- `call_tool` param & result types: `src/service/client.rs` (`method! ... call_tool CallToolRequest(CallToolRequestParams) => CallToolResult`) and `src/model/tool.rs`.
- In-process transport via `(R, W)`: `src/transport/async_rw.rs` (`impl ... IntoTransport for (R, W)`; `tokio::io::duplex` used in its tests).
- `ErrorData::{internal_error, invalid_request}`: `src/model.rs`.

The code below is written to this API; if the compiler rejects an exact name (e.g. `CallToolRequestParam` vs `CallToolRequestParams`, or an `arguments` field type), adjust to the installed symbol and note it in the report. Do **not** change the gate semantics.

- [ ] **Step 1: Add dependencies** — edit `crates/kontur-mcp/Cargo.toml`, add to `[dependencies]`:

```toml
rmcp = { version = "2.2", features = ["server", "client", "macros", "schemars", "transport-async-rw"] }
serde_json = "1"
```

- [ ] **Step 2: Create `crates/kontur-mcp/src/server.rs`**

```rust
use std::sync::Arc;

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData};
use serde::{Deserialize, Serialize};

use kontur_core::{HoldState, Remedy, TaskId};

use crate::gatehost::GateHost;

/// The rmcp server exposing the agent-facing gated tools over a `GateHost`.
#[derive(Clone)]
pub struct KonturServer {
    host: Arc<GateHost>,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
pub struct WriteFileInput {
    pub task_id: String,
    pub path: String,
    pub contents: String,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
pub struct OkOutput {
    pub ok: bool,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
pub struct RunCommandInput {
    pub task_id: String,
    pub command: String,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
pub struct CommandOut {
    pub stdout: String,
    pub exit_code: i32,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
pub struct ProposeInput {
    pub task_id: String,
    #[serde(default)]
    pub tokens: u64,
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
pub struct ProposeOutput {
    pub accepted: bool,
    pub reviewed_by: Vec<String>,
}

impl KonturServer {
    pub fn new(host: Arc<GateHost>) -> Self {
        KonturServer { host }
    }
}

#[tool_router(server_handler)]
impl KonturServer {
    #[tool(name = "write_file", description = "Write a file in the agent's worktree (recorded, not gated).")]
    async fn write_file(
        &self,
        Parameters(WriteFileInput { task_id, path, contents }): Parameters<WriteFileInput>,
    ) -> Result<Json<OkOutput>, ErrorData> {
        self.host
            .record_write(&TaskId(task_id), &path, contents.as_bytes())
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(Json(OkOutput { ok: true }))
    }

    #[tool(name = "run_command", description = "Run a command in the agent's worktree (recorded, not gated).")]
    async fn run_command(
        &self,
        Parameters(RunCommandInput { task_id, command, cwd }): Parameters<RunCommandInput>,
    ) -> Result<Json<CommandOut>, ErrorData> {
        let out = self
            .host
            .run_command(&TaskId(task_id), &command, &cwd)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(Json(CommandOut { stdout: out.stdout, exit_code: out.exit_code }))
    }

    #[tool(name = "propose_task_complete", description = "Submit the completed task for four-eyes review; blocks until both operators sign off.")]
    async fn propose_task_complete(
        &self,
        Parameters(ProposeInput { task_id, tokens }): Parameters<ProposeInput>,
    ) -> Result<Json<ProposeOutput>, ErrorData> {
        let task_id = TaskId(task_id);
        let (gate_id, mut rx) = self
            .host
            .begin_task_gate(task_id, tokens)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // Await resolution. `borrow_and_update` reads the latest state; loop
        // until terminal. A closed channel means the session is shutting down.
        loop {
            let state = *rx.borrow_and_update();
            if matches!(state, HoldState::Satisfied | HoldState::Blocked) {
                break;
            }
            if rx.changed().await.is_err() {
                return Err(ErrorData::internal_error("session closed before gate resolved", None));
            }
        }

        let final_ = self
            .host
            .gate_outcome(&gate_id)
            .await
            .ok_or_else(|| ErrorData::internal_error("gate disappeared", None))?;

        match final_.state {
            HoldState::Satisfied => Ok(Json(ProposeOutput {
                accepted: true,
                reviewed_by: final_.reviewed_by.iter().map(|o| hex32(&o.0)).collect(),
            })),
            HoldState::Blocked => {
                let remedy = match final_.remedy {
                    Some(Remedy::Steer(s)) => s,
                    Some(Remedy::HandEdit(h)) => format!("hand-edit:{}", h.0),
                    None => "blocked".to_string(),
                };
                Err(ErrorData::invalid_request(format!("task rejected: {remedy}"), None))
            }
            other => Err(ErrorData::internal_error(format!("non-terminal gate state: {other:?}"), None)),
        }
    }
}

fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
```

- [ ] **Step 3: Wire the module** — edit `crates/kontur-mcp/src/lib.rs`, add `pub mod server;` and `pub use server::KonturServer;`.

- [ ] **Step 4: Compile-check the server before the e2e test**

Run: `cargo build -p kontur-mcp 2>&1`
Expected: compiles. If the `#[tool_router(server_handler)]` macro needs a different invocation, or `Parameters`/`Json`/`ErrorData` live at a different path, reconcile against the grounding files above and re-run. Zero warnings.

- [ ] **Step 5: Write the failing end-to-end test** — create `crates/kontur-mcp/tests/server_mcp.rs`

```rust
use std::sync::Arc;
use std::time::Duration;

use kontur_core::{
    CastVerdict, Ed25519Signer, FixedClock, GateId, Hash, HoldState, ReviewDepth, Signer, TaskId, Verdict,
};
use kontur_mcp::{GateHost, InMemoryWorkspace, KonturServer, SessionContext};

use rmcp::model::CallToolRequestParam;
use rmcp::{serve_server, ServiceExt};

fn go(seed: u8, gate_id: &GateId, dh: Hash) -> CastVerdict {
    let signer = Ed25519Signer::from_seed([seed; 32]);
    CastVerdict::create(&signer, &FixedClock(1000 + seed as i64), gate_id, dh, Verdict::Go, ReviewDepth::FullDiff, None)
}

/// Poll the operator face until a gate appears (the agent-side handler opens it
/// asynchronously). Bounded so a bug fails fast instead of hanging.
async fn wait_for_gate(host: &GateHost) -> (GateId, Hash) {
    for _ in 0..2000 {
        if let Some(v) = host.pending_gates().await.into_iter().next() {
            return (v.gate_id, v.diff_hash);
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("no gate appeared");
}

#[tokio::test]
async fn agent_write_then_propose_gated_by_two_operators() {
    let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
    let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
    let ws = Arc::new(InMemoryWorkspace::new());
    let ctx = SessionContext::new("refactor guard", op1, "agent-01", "claude", "1.0", vec![op1, op2]);
    let host = Arc::new(GateHost::new(ctx, ws.clone()));

    // Wire an in-process client<->server over a duplex pipe.
    let (server_io, client_io) = tokio::io::duplex(8192);
    let (sr, sw) = tokio::io::split(server_io);
    let (cr, cw) = tokio::io::split(client_io);

    let server = KonturServer::new(host.clone());
    tokio::spawn(async move {
        // Ignore the join result; the test drives lifetime via the client.
        let _ = serve_server(server, (sr, sw)).await;
    });
    let client = ().serve((cr, cw)).await.expect("client handshake");

    // 1) write_file — ungated, executes in the workspace.
    let write_args = serde_json::json!({ "task_id": "t1", "path": "a.rs", "contents": "guarded\n" });
    client
        .call_tool(CallToolRequestParam {
            name: "write_file".into(),
            arguments: write_args.as_object().cloned(),
        })
        .await
        .expect("write_file call");
    assert_eq!(ws.file_contents(&TaskId("t1".into()), "a.rs"), Some(b"guarded\n".to_vec()));

    // 2) propose_task_complete — blocks; drive it on a task.
    let client2 = client.clone();
    let propose = tokio::spawn(async move {
        client2
            .call_tool(CallToolRequestParam {
                name: "propose_task_complete".into(),
                arguments: serde_json::json!({ "task_id": "t1", "tokens": 42 }).as_object().cloned(),
            })
            .await
    });

    // 3) Two operators sign off via the operator face.
    let (gate_id, dh) = wait_for_gate(&host).await;
    host.submit_verdict(&gate_id, go(1, &gate_id, dh)).await.unwrap();
    host.submit_verdict(&gate_id, go(2, &gate_id, dh)).await.unwrap();

    // 4) The blocked tool call now returns success, and the audit chain holds.
    let result = propose.await.expect("join").expect("propose call ok");
    assert_eq!(result.is_error, Some(false));
    assert!(host.verify_audit().await.is_ok());
    assert_eq!(ws.accepted_tasks(), vec![TaskId("t1".into())]);
    assert_eq!(host.reviewed_by(&gate_id).await.unwrap().len(), 2);
}
```

> **Adaptation note:** the client value from `().serve(transport)` is a `RunningService` whose `call_tool` comes from the peer/`ServiceExt` surface. If `call_tool` is reached via `client.peer().call_tool(...)` or the param/result names differ in the installed 2.2.0 (`CallToolRequestParam` vs `...Params`, `is_error` vs another field on `CallToolResult`), reconcile against `src/service/client.rs` and `src/model/tool.rs` (paths in the grounding block) and adjust — keep the assertions' intent (call succeeds, chain verifies, task accepted, two reviewers).

- [ ] **Step 6: Run the e2e test**

Run: `cargo test -p kontur-mcp --test server_mcp`
Expected: PASS (1 test).

- [ ] **Step 7: Full suite + clippy**

Run: `cargo test -p kontur-mcp && cargo clippy -p kontur-mcp --all-targets -- -D warnings`
Expected: all tests PASS; clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/kontur-mcp/Cargo.toml crates/kontur-mcp/src/server.rs crates/kontur-mcp/src/lib.rs crates/kontur-mcp/tests/server_mcp.rs Cargo.lock
git commit -m "feat(mcp): rmcp server + in-process end-to-end gate test

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (for the executor)

- **Spec coverage:** gate host orchestrator (Tasks 4–6), real MCP server (Task 8), `Workspace` port + fs impl (Tasks 2, 7), audit emission + `reviewed_by` (Tasks 4–5), two faces (agent = Task 8 tools; operator = `submit_verdict`/`hand_edit`/`pending_gates`, Tasks 4–6), blind-sealing preserved on the operator face (Task 5 `pending_gates` test), no single-key accept (Task 4/5 — `accept_task` only on `Satisfied`), hand-edit + escalation (Task 6), park-on-loss (the `propose` handler simply keeps awaiting — Task 8).
- **Deferred (not this plan):** Claude Code binding, network/attach, TUI, real git commit/merge (behind `accept_task`), escalation timers. Recorded in the spec §1.
- **rmcp risk is isolated to Task 8** and grounded against the installed source path; Tasks 1–7 have no rmcp dependency and are fully deterministic.
- **Known adaptation points in Task 8** (flagged inline): exact `call_tool` param/result symbol names and the client `serve`/`peer` surface in rmcp 2.2.0 — reconcile against the cited source files, preserving gate semantics.
```
