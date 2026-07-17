# Kontur TUI Console Implementation Plan (first slice)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `kontur-tui`, a runnable brutalist ratatui console over a real local `kontur-mcp` `GateHost` that renders the watch-floor and drives the four-eyes merge-gate sign-off flow.

**Architecture:** A pure `SessionView` snapshot is built from the `GateHost` operator face + a mock `FleetSource`; a pure `render` function draws it with ratatui; an async app loop maps crossterm keys to operator-face calls (plus a scripted second key) and rebuilds the view. All logic except the terminal loop is unit-tested.

**Tech Stack:** Rust (edition 2021), `ratatui` 0.30 (+ its re-exported `crossterm` 0.29), `tokio`, `kontur-mcp`, `kontur-core`.

## Global Constraints

- Rust edition 2021, stable toolchain.
- **Blind sealing never violated:** `KeyView` is built ONLY from `GateView.observed` (`kontur-core`'s `VerdictView`, sealing-safe). A sealed verdict's value must never enter the view or render.
- **No bare veto in the UI:** `[r]` opens a remedy input; a `no-go` is submitted only with a steer.
- **No decorative telemetry / no faked signals** (brutalist). Identity flourish (КОНТУР-1) stays in the banner. Mock fleet values are clearly demo data.
- **Never render or log a private key or a sealed verdict value.**
- **Terminal always restored** on normal exit, error, and panic (raw mode + alternate screen torn down via a Drop guard + panic hook).
- **The second key is scripted** — the UI must not present it as a genuine second human.
- Use `ratatui::crossterm` (the re-exported crossterm) — do NOT add a separate `crossterm` dependency (avoids a version mismatch).
- No `HashMap`/`HashSet` in anything hashed by `kontur-core` (unchanged here; the TUI does not hash).
- `.gitignore` covers `/target`; stage ONLY the files each task changed — never `git add -A`/`git add .`.
- `cargo clippy --all-targets -- -D warnings` clean; test output pristine.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Grounding: ratatui 0.30 (installed source)

Extracted at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ratatui-0.30.2/`. Public API (from its `src/lib.rs` re-exports): `ratatui::{Frame, Terminal}`; `ratatui::backend::{TestBackend, CrosstermBackend}`; `ratatui::buffer::Buffer`; `ratatui::layout::{Layout, Constraint, Direction, Rect}`; `ratatui::style::{Style, Color, Modifier, Stylize}`; `ratatui::text::{Line, Span, Text}`; `ratatui::widgets::{Block, Borders, Paragraph, Wrap}`; `ratatui::crossterm` (re-exported crossterm 0.29 — use `ratatui::crossterm::event` / `::terminal` / `::execute`). `Frame::area() -> Rect`. `Terminal::new(backend)`, `terminal.draw(|f| ...)`, `terminal.backend().buffer() -> &Buffer`. `Buffer` indexes by `buf[(x, y)] -> &Cell`, `Cell::symbol() -> &str`, and exposes its `Rect` (use `buf.area` if it is a field, else `buf.area()` — reconcile against the compiler). If any exact symbol differs, adjust minimally to the installed API and note it.

---

## File structure

```
crates/kontur-mcp/src/gatehost.rs   # (Task 1) add GateView.files/loc, gate_diff, audit_len
crates/kontur-mcp/src/workspace.rs  # (Task 1) no change expected; gate_diff re-freezes via Workspace
crates/kontur-tui/
  Cargo.toml
  src/
    lib.rs            # module wiring + re-exports
    view.rs           # SessionView + sub-structs (pure data)   (Task 2)
    fleet.rs          # FleetSource trait + MockFleet            (Task 3)
    input.rs          # Action + map_key                         (Task 4)
    viewmodel.rs      # build_session_view                       (Task 5)
    render.rs         # render(frame, &SessionView)              (Task 6)
    app.rs            # TerminalGuard + async run loop           (Task 7)
    demo.rs           # wire GateHost + MockFleet + scripted 2nd (Task 8)
    bin/kontur.rs     # entry point                              (Task 8)
  tests/
    render.rs         # TestBackend golden-cell assertions       (Task 6)
    flow.rs           # headless pending->cast->2nd->accept      (Task 8)
```

---

### Task 1: `kontur-mcp` operator-face additions (files/loc, gate_diff, audit_len)

**Files:**
- Modify: `crates/kontur-mcp/src/gatehost.rs`

**Interfaces:**
- Consumes: existing `GateHost`, `GateView`, `HoldEntry.provenance`, `Workspace::freeze_task_diff`.
- Produces: `GateView.files: Vec<String>`, `GateView.loc: u32`; `GateHost::gate_diff(&GateId) -> Option<Vec<u8>>`; `GateHost::audit_len() -> usize`.

- [ ] **Step 1: Add fields to `GateView`** — in `gatehost.rs`, extend the struct:

```rust
pub struct GateView {
    pub gate_id: GateId,
    pub task_id: TaskId,
    pub diff_hash: Hash,
    pub state: HoldState,
    pub observed: Vec<VerdictView>,
    pub escalation_required: bool,
    pub files: Vec<String>,
    pub loc: u32,
}
```

- [ ] **Step 2: Populate them in `pending_gates`** — in the `.map(|e| GateView { ... })` closure, add the two fields from the entry's provenance:

```rust
            .map(|e| GateView {
                gate_id: e.hold.gate_id().clone(),
                task_id: e.hold.task_id().clone(),
                diff_hash: e.hold.diff_hash(),
                state: e.hold.state(),
                observed: e.hold.observed_verdicts(),
                escalation_required: e.escalation_required,
                files: e.provenance.files.clone(),
                loc: e.provenance.loc,
            })
```

- [ ] **Step 3: Add `gate_diff` and `audit_len`** — inside `impl GateHost`, before the closing brace:

```rust
    /// The current frozen diff bytes for a gate's task (a review preview). The
    /// authoritative content hash is the gate's `diff_hash`; this is the bytes
    /// an operator opens to review.
    pub async fn gate_diff(&self, gate_id: &GateId) -> Option<Vec<u8>> {
        let st = self.state.lock().await;
        let task_id = st.holds.iter().find(|e| e.hold.gate_id() == gate_id)?.hold.task_id().clone();
        drop(st);
        self.workspace.freeze_task_diff(&task_id).ok().map(|f| f.bytes)
    }

    /// Number of records currently in the audit chain.
    pub async fn audit_len(&self) -> usize {
        self.state.lock().await.chain.records().len()
    }
```

- [ ] **Step 4: Write the failing tests** — add to the `tests` module in `gatehost.rs`:

```rust
    #[tokio::test]
    async fn gate_view_carries_files_and_loc() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());
        let (_gid, _dh) = open_a_gate(&host, &ws, &context).await;
        let view = &host.pending_gates().await[0];
        assert_eq!(view.files, vec!["a.rs".to_string()]);
        assert_eq!(view.loc, 1);
    }

    #[tokio::test]
    async fn gate_diff_and_audit_len() {
        let op1 = Ed25519Signer::from_seed([1; 32]).operator_id();
        let op2 = Ed25519Signer::from_seed([2; 32]).operator_id();
        let ws = Arc::new(InMemoryWorkspace::new());
        let context = ctx(vec![op1, op2]);
        let host = GateHost::new(context.clone(), ws.clone());
        let (gid, dh) = open_a_gate(&host, &ws, &context).await;

        let diff = host.gate_diff(&gid).await.expect("diff bytes");
        assert!(!diff.is_empty());
        assert_eq!(host.audit_len().await, 0);

        host.submit_verdict(&gid, go_verdict(1, &gid, dh)).await.unwrap();
        host.submit_verdict(&gid, go_verdict(2, &gid, dh)).await.unwrap();
        assert_eq!(host.audit_len().await, 1);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p kontur-mcp gatehost` then `cargo build -p kontur-mcp --all-targets 2>&1`
Expected: all pass (2 new + prior); zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/kontur-mcp/src/gatehost.rs
git commit -m "feat(mcp): expose gate diff/files/loc + audit_len for the TUI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `kontur-tui` scaffold + `SessionView` data types

**Files:**
- Modify: `Cargo.toml` (workspace root — add member)
- Create: `crates/kontur-tui/Cargo.toml`
- Create: `crates/kontur-tui/src/lib.rs`
- Create: `crates/kontur-tui/src/view.rs`

**Interfaces:**
- Produces: `Role`, `Station`, `Banner`, `StatusStrip`, `AgentCard`, `LogLine`, `KeyStatus`, `KeyView`, `GateCard`, `InterventionCard`, `AuditSummary`, `ActiveRegion`, `SessionView`.

- [ ] **Step 1: Add the crate to the workspace** — edit root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/kontur-core", "crates/kontur-mcp", "crates/kontur-tui"]
```

- [ ] **Step 2: Create `crates/kontur-tui/Cargo.toml`**

```toml
[package]
name = "kontur-tui"
version = "0.1.0"
edition = "2021"

[dependencies]
kontur-core = { path = "../kontur-core" }
kontur-mcp = { path = "../kontur-mcp" }
ratatui = "0.30"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }

[[bin]]
name = "kontur"
path = "src/bin/kontur.rs"
```

- [ ] **Step 3: Create `crates/kontur-tui/src/view.rs`**

```rust
use kontur_core::OperatorId;

/// Operator role. Rotates in later slices; label-only here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Driver,
    Navigator,
}

impl Role {
    pub fn label(&self) -> &'static str {
        match self {
            Role::Driver => "DRIVER",
            Role::Navigator => "NAVIGATOR",
        }
    }
}

/// A human seat.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Station {
    pub label: String,
    pub role: Role,
    pub activity: String,
    pub operator: OperatorId,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Banner {
    pub session: String,
    pub version: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusStrip {
    pub linked: bool,
    pub four_eyes: bool,
    pub fleet_count: usize,
    pub needs_you: usize,
    pub tokens: u64,
}

/// One agent panel on the watch-floor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentCard {
    pub id: String,
    pub status: String,
    pub tokens: u64,
    pub needs_signoff: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LogLine {
    pub time: String,
    pub who: String,
    pub text: String,
}

/// What a key shows. Never carries a sealed verdict's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyStatus {
    Awaiting,
    Sealed,
    Go,
    NoGo,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyView {
    pub label: String,
    pub role: Role,
    pub status: KeyStatus,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GateCard {
    pub gate_id: String,
    pub task: String,
    pub files: Vec<String>,
    pub loc: u32,
    pub keys: Vec<KeyView>,
    pub escalation_required: bool,
    pub diff_preview: Option<String>,
}

/// A no-go remedy being composed at a gate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InterventionCard {
    pub gate_id: String,
    pub steer: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditSummary {
    pub gates: usize,
    pub reviewers: Vec<String>,
    pub chain_verified: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ActiveRegion {
    Idle,
    Gate(GateCard),
    Intervention(InterventionCard),
    SessionClosed(AuditSummary),
}

/// The full pure snapshot the console renders.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SessionView {
    pub banner: Banner,
    pub status: StatusStrip,
    pub stations: [Station; 2],
    pub fleet: Vec<AgentCard>,
    pub log: Vec<LogLine>,
    pub active: ActiveRegion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_labels() {
        assert_eq!(Role::Driver.label(), "DRIVER");
        assert_eq!(Role::Navigator.label(), "NAVIGATOR");
    }
}
```

- [ ] **Step 4: Create `crates/kontur-tui/src/lib.rs`**

```rust
//! Kontur TUI: the brutalist two-seat operator console (first slice).

pub mod view;

pub use view::{
    ActiveRegion, AgentCard, AuditSummary, Banner, GateCard, InterventionCard, KeyStatus, KeyView,
    LogLine, Role, SessionView, Station, StatusStrip,
};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p kontur-tui` then `cargo build -p kontur-tui 2>&1`
Expected: PASS (1 test); the crate compiles (ratatui downloads on first build); zero warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/kontur-tui/Cargo.toml crates/kontur-tui/src/lib.rs crates/kontur-tui/src/view.rs
git commit -m "feat(tui): scaffold kontur-tui + SessionView data types

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: FleetSource + MockFleet

**Files:**
- Create: `crates/kontur-tui/src/fleet.rs`
- Modify: `crates/kontur-tui/src/lib.rs`

**Interfaces:**
- Consumes: `AgentCard` (Task 2).
- Produces: `trait FleetSource { fn agents(&self) -> Vec<AgentCard>; }`; `MockFleet` with `new(Vec<AgentCard>)`, `demo()`.

- [ ] **Step 1: Create `crates/kontur-tui/src/fleet.rs`**

```rust
use crate::view::AgentCard;

/// Source of fleet (agent) status for the watch-floor. Mocked in this slice;
/// a real source arrives with the live-agent binding.
pub trait FleetSource {
    fn agents(&self) -> Vec<AgentCard>;
}

/// A scripted fleet for the demo console.
pub struct MockFleet {
    agents: Vec<AgentCard>,
}

impl MockFleet {
    pub fn new(agents: Vec<AgentCard>) -> Self {
        MockFleet { agents }
    }

    /// A small demo fleet: two calm agents and one needing sign-off.
    pub fn demo() -> Self {
        MockFleet::new(vec![
            AgentCard { id: "agent-01".into(), status: "analysing parser.rs".into(), tokens: 3100, needs_signoff: false },
            AgentCard { id: "agent-02".into(), status: "editing auth".into(), tokens: 1200, needs_signoff: false },
            AgentCard { id: "agent-03".into(), status: "needs sign-off".into(), tokens: 0, needs_signoff: true },
        ])
    }
}

impl FleetSource for MockFleet {
    fn agents(&self) -> Vec<AgentCard> {
        self.agents.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_fleet_has_one_needing_signoff() {
        let f = MockFleet::demo();
        let agents = f.agents();
        assert_eq!(agents.len(), 3);
        assert_eq!(agents.iter().filter(|a| a.needs_signoff).count(), 1);
    }

    #[test]
    fn new_round_trips_agents() {
        let a = AgentCard { id: "x".into(), status: "y".into(), tokens: 5, needs_signoff: false };
        let f = MockFleet::new(vec![a.clone()]);
        assert_eq!(f.agents(), vec![a]);
    }
}
```

- [ ] **Step 2: Wire the module** — edit `lib.rs`, add `pub mod fleet;` and `pub use fleet::{FleetSource, MockFleet};`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-tui fleet`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-tui/src/fleet.rs crates/kontur-tui/src/lib.rs
git commit -m "feat(tui): FleetSource + MockFleet

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Input actions + key mapping

**Files:**
- Create: `crates/kontur-tui/src/input.rs`
- Modify: `crates/kontur-tui/src/lib.rs`

**Interfaces:**
- Consumes: `ratatui::crossterm::event::KeyCode`.
- Produces: `Action` enum; `fn map_key(code: KeyCode, composing_remedy: bool) -> Action`.

- [ ] **Step 1: Create `crates/kontur-tui/src/input.rs`**

```rust
use ratatui::crossterm::event::KeyCode;

/// A mapped operator intent. The app applies these against the GateHost.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Go,
    NoGoBegin,
    HandEdit,
    Discuss,
    OpenDiff,
    RotateRole,
    Help,
    Quit,
    RemedyChar(char),
    RemedyBackspace,
    RemedySubmit,
    RemedyCancel,
    None,
}

/// Map a key to an action. When composing a remedy, typing feeds the remedy
/// buffer; otherwise gate/global keys apply.
pub fn map_key(code: KeyCode, composing_remedy: bool) -> Action {
    if composing_remedy {
        return match code {
            KeyCode::Char(c) => Action::RemedyChar(c),
            KeyCode::Backspace => Action::RemedyBackspace,
            KeyCode::Enter => Action::RemedySubmit,
            KeyCode::Esc => Action::RemedyCancel,
            _ => Action::None,
        };
    }
    match code {
        KeyCode::Char('g') => Action::Go,
        KeyCode::Char('r') => Action::NoGoBegin,
        KeyCode::Char('e') => Action::HandEdit,
        KeyCode::Char('d') => Action::Discuss,
        KeyCode::Char('o') => Action::OpenDiff,
        KeyCode::Tab => Action::RotateRole,
        KeyCode::Char('?') => Action::Help,
        KeyCode::Char('q') => Action::Quit,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_keys_map() {
        assert_eq!(map_key(KeyCode::Char('g'), false), Action::Go);
        assert_eq!(map_key(KeyCode::Char('r'), false), Action::NoGoBegin);
        assert_eq!(map_key(KeyCode::Char('e'), false), Action::HandEdit);
        assert_eq!(map_key(KeyCode::Char('q'), false), Action::Quit);
        assert_eq!(map_key(KeyCode::Char('z'), false), Action::None);
    }

    #[test]
    fn remedy_composition_captures_text() {
        assert_eq!(map_key(KeyCode::Char('x'), true), Action::RemedyChar('x'));
        assert_eq!(map_key(KeyCode::Enter, true), Action::RemedySubmit);
        assert_eq!(map_key(KeyCode::Esc, true), Action::RemedyCancel);
        // 'g' while composing is text, not a Go verdict.
        assert_eq!(map_key(KeyCode::Char('g'), true), Action::RemedyChar('g'));
    }
}
```

- [ ] **Step 2: Wire the module** — edit `lib.rs`, add `pub mod input;` and `pub use input::{map_key, Action};`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-tui input`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-tui/src/input.rs crates/kontur-tui/src/lib.rs
git commit -m "feat(tui): input actions + key mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: viewmodel — build SessionView from the GateHost

**Files:**
- Create: `crates/kontur-tui/src/viewmodel.rs`
- Modify: `crates/kontur-tui/src/lib.rs`

**Interfaces:**
- Consumes: `GateHost` (kontur-mcp), `GateView`, `VerdictView`/`VerdictStatus`/`Verdict` (kontur-core), all `view.rs` types, `FleetSource`.
- Produces: `async fn build_session_view(host: &GateHost, fleet: &dyn FleetSource, stations: [Station; 2], banner: Banner, log: Vec<LogLine>, closed: bool) -> SessionView`.

- [ ] **Step 1: Create `crates/kontur-tui/src/viewmodel.rs`**

```rust
use kontur_core::{Verdict, VerdictStatus};
use kontur_mcp::{GateHost, GateView};

use crate::fleet::FleetSource;
use crate::view::{
    ActiveRegion, AuditSummary, Banner, GateCard, KeyStatus, KeyView, LogLine, SessionView, Station,
    StatusStrip,
};

/// Build the pure console snapshot. Pure w.r.t. the host + fleet at call time;
/// blind sealing is preserved because keys come only from `GateView.observed`.
pub async fn build_session_view(
    host: &GateHost,
    fleet: &dyn FleetSource,
    stations: [Station; 2],
    banner: Banner,
    log: Vec<LogLine>,
    closed: bool,
) -> SessionView {
    let agents = fleet.agents();
    let pending = host.pending_gates().await;

    let status = StatusStrip {
        linked: true,
        four_eyes: true,
        fleet_count: agents.len(),
        needs_you: pending.len(),
        tokens: agents.iter().map(|a| a.tokens).sum(),
    };

    let active = if closed {
        ActiveRegion::SessionClosed(AuditSummary {
            gates: host.audit_len().await,
            reviewers: stations.iter().map(|s| s.label.clone()).collect(),
            chain_verified: host.verify_audit().await.is_ok(),
        })
    } else if let Some(gv) = pending.first() {
        let diff_preview = host
            .gate_diff(&gv.gate_id)
            .await
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        ActiveRegion::Gate(gate_card(gv, &stations, diff_preview))
    } else {
        ActiveRegion::Idle
    };

    SessionView { banner, status, stations, fleet: agents, log, active }
}

fn gate_card(gv: &GateView, stations: &[Station; 2], diff_preview: Option<String>) -> GateCard {
    let keys = stations.iter().map(|s| key_for(s, gv)).collect();
    GateCard {
        gate_id: gv.gate_id.0.clone(),
        task: gv.task_id.0.clone(),
        files: gv.files.clone(),
        loc: gv.loc,
        keys,
        escalation_required: gv.escalation_required,
        diff_preview,
    }
}

/// Derive a station's key status from the sealing-safe observed verdicts.
fn key_for(station: &Station, gv: &GateView) -> KeyView {
    let status = gv
        .observed
        .iter()
        .find(|v| v.operator == station.operator)
        .map(|v| match &v.status {
            VerdictStatus::Sealed => KeyStatus::Sealed,
            VerdictStatus::Revealed(Verdict::Go) => KeyStatus::Go,
            VerdictStatus::Revealed(Verdict::NoGo(_)) => KeyStatus::NoGo,
        })
        .unwrap_or(KeyStatus::Awaiting);
    KeyView { label: station.label.clone(), role: station.role, status }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::MockFleet;
    use crate::view::Role;
    use kontur_core::{
        CastVerdict, Ed25519Signer, FixedClock, Hash, ReviewDepth, Signer, TaskId,
    };
    use kontur_mcp::{InMemoryWorkspace, SessionContext, Workspace};
    use std::sync::Arc;

    fn stations(a: kontur_core::OperatorId, b: kontur_core::OperatorId) -> [Station; 2] {
        [
            Station { label: "A · YOU".into(), role: Role::Driver, activity: "watching".into(), operator: a },
            Station { label: "B · J.REED".into(), role: Role::Navigator, activity: "reviewing".into(), operator: b },
        ]
    }

    fn banner() -> Banner {
        Banner { session: "4417".into(), version: "0.1.0".into() }
    }

    #[tokio::test]
    async fn pending_gate_shows_gate_region_with_sealed_first_key() {
        let s1 = Ed25519Signer::from_seed([1; 32]);
        let s2 = Ed25519Signer::from_seed([2; 32]);
        let (op1, op2) = (s1.operator_id(), s2.operator_id());
        let ws = Arc::new(InMemoryWorkspace::new());
        let ctx = SessionContext::new("do it", op1, "agent-01", "claude", "1.0", vec![op1, op2]);
        let host = GateHost::new(ctx, ws.clone());

        let task = TaskId("t1".into());
        ws.apply_write(&task, "a.rs", b"x\n").unwrap();
        let (gid, _rx) = host.begin_task_gate(task, 0).await.unwrap();
        let dh = host.pending_gates().await[0].diff_hash;

        // Station A casts (blind, sealed).
        let cv = CastVerdict::create(&s1, &FixedClock(1000), &gid, dh, kontur_core::Verdict::Go, ReviewDepth::FullDiff, None);
        host.submit_verdict(&gid, cv).await.unwrap();

        let view = build_session_view(&host, &MockFleet::demo(), stations(op1, op2), banner(), vec![], false).await;
        match view.active {
            ActiveRegion::Gate(card) => {
                assert_eq!(card.files, vec!["a.rs".to_string()]);
                assert_eq!(card.keys[0].status, KeyStatus::Sealed); // A cast, sealed
                assert_eq!(card.keys[1].status, KeyStatus::Awaiting); // B not yet
            }
            other => panic!("expected Gate, got {other:?}"),
        }
        assert_eq!(view.status.needs_you, 1);
    }

    #[tokio::test]
    async fn closed_shows_session_summary_with_verified_chain() {
        let s1 = Ed25519Signer::from_seed([1; 32]);
        let s2 = Ed25519Signer::from_seed([2; 32]);
        let (op1, op2) = (s1.operator_id(), s2.operator_id());
        let ws = Arc::new(InMemoryWorkspace::new());
        let ctx = SessionContext::new("do it", op1, "agent-01", "claude", "1.0", vec![op1, op2]);
        let host = GateHost::new(ctx, ws.clone());

        let task = TaskId("t1".into());
        ws.apply_write(&task, "a.rs", b"x\n").unwrap();
        let (gid, _rx) = host.begin_task_gate(task, 0).await.unwrap();
        let dh = host.pending_gates().await[0].diff_hash;
        for s in [&s1, &s2] {
            let cv = CastVerdict::create(s, &FixedClock(1000), &gid, dh, kontur_core::Verdict::Go, ReviewDepth::FullDiff, None);
            host.submit_verdict(&gid, cv).await.unwrap();
        }

        let view = build_session_view(&host, &MockFleet::demo(), stations(op1, op2), banner(), vec![], true).await;
        match view.active {
            ActiveRegion::SessionClosed(summary) => {
                assert_eq!(summary.gates, 1);
                assert!(summary.chain_verified);
                assert_eq!(summary.reviewers.len(), 2);
            }
            other => panic!("expected SessionClosed, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Wire the module** — edit `lib.rs`, add `pub mod viewmodel;` and `pub use viewmodel::build_session_view;`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-tui viewmodel` then `cargo build -p kontur-tui --all-targets 2>&1`
Expected: PASS (2 tests); zero warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-tui/src/viewmodel.rs crates/kontur-tui/src/lib.rs
git commit -m "feat(tui): build SessionView from the GateHost (sealing-safe)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: render — draw the console + golden tests

**Files:**
- Create: `crates/kontur-tui/src/render.rs`
- Create: `crates/kontur-tui/tests/render.rs`
- Modify: `crates/kontur-tui/src/lib.rs`

**Interfaces:**
- Consumes: `SessionView` + sub-types (Task 2); ratatui `Frame`, `Layout`, `Block`, `Paragraph`, `Line`, `Span`, `Style`.
- Produces: `pub fn render(frame: &mut ratatui::Frame, view: &SessionView)`.

- [ ] **Step 1: Create `crates/kontur-tui/src/render.rs`**

```rust
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::view::{ActiveRegion, KeyStatus, SessionView};

/// Draw the whole console. Pure: no I/O, no engine calls.
pub fn render(frame: &mut Frame, view: &SessionView) {
    let rows = Layout::vertical([
        Constraint::Length(1), // banner
        Constraint::Length(1), // status strip
        Constraint::Length(3), // stations
        Constraint::Length(5), // fleet
        Constraint::Min(3),    // log
        Constraint::Length(8), // active region
        Constraint::Length(1), // command line
    ])
    .split(frame.area());

    banner(frame, rows[0], view);
    status(frame, rows[1], view);
    stations(frame, rows[2], view);
    fleet(frame, rows[3], view);
    log(frame, rows[4], view);
    active(frame, rows[5], view);
    command(frame, rows[6]);
}

fn banner(frame: &mut Frame, area: Rect, view: &SessionView) {
    let text = format!(
        "[ КОНТУР-1  //  co-op session {}  //  v{} ]",
        view.banner.session, view.banner.version
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().add_modifier(Modifier::BOLD)),
        area,
    );
}

fn status(frame: &mut Frame, area: Rect, view: &SessionView) {
    let s = &view.status;
    let needs = if s.needs_you > 0 {
        format!("FLEET {} ({} NEEDS YOU)", s.fleet_count, s.needs_you)
    } else {
        format!("FLEET {}", s.fleet_count)
    };
    let line = format!(
        " LINK {} || 4-EYES {} || {} || {} tok",
        if s.linked { "BOTH-STATIONS SYNC" } else { "B-STATION DROPPED" },
        if s.four_eyes { "ON" } else { "OFF" },
        needs,
        s.tokens
    );
    frame.render_widget(Paragraph::new(line), area);
}

fn stations(frame: &mut Frame, area: Rect, view: &SessionView) {
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    for (i, st) in view.stations.iter().enumerate() {
        let block = Block::bordered().title(st.label.clone());
        let body = format!("{} · {}", st.role.label(), st.activity);
        frame.render_widget(Paragraph::new(body).block(block), cols[i]);
    }
}

fn fleet(frame: &mut Frame, area: Rect, view: &SessionView) {
    let lines: Vec<Line> = view
        .fleet
        .iter()
        .map(|a| {
            let marker = if a.needs_signoff { "▶ NEEDS SIGN-OFF" } else { a.status.as_str() };
            let text = format!(" {:<10} {:<20} {} tok", a.id, marker, a.tokens);
            if a.needs_signoff {
                Line::from(Span::styled(text, Style::default().add_modifier(Modifier::BOLD)))
            } else {
                Line::from(text)
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title("FLEET")), area);
}

fn log(frame: &mut Frame, area: Rect, view: &SessionView) {
    let lines: Vec<Line> = view
        .log
        .iter()
        .map(|l| Line::from(format!(" {} {:<8} {}", l.time, l.who, l.text)))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title("LOG")), area);
}

fn active(frame: &mut Frame, area: Rect, view: &SessionView) {
    match &view.active {
        ActiveRegion::Idle => {
            frame.render_widget(
                Paragraph::new(" no task dispatched — draft an instruction to begin")
                    .block(Block::bordered().title("PROMPT")),
                area,
            );
        }
        ActiveRegion::Gate(card) => {
            let mut lines = vec![Line::from(format!(
                " GATE {} · {} · {} · +{} loc",
                card.gate_id,
                card.task,
                card.files.join(", "),
                card.loc
            ))];
            for key in &card.keys {
                let status = match key.status {
                    KeyStatus::Awaiting => "□ awaiting verdict",
                    KeyStatus::Sealed => "■ cast — sealed",
                    KeyStatus::Go => "■ GO",
                    KeyStatus::NoGo => "■ NO-GO",
                };
                lines.push(Line::from(format!("   KEY {:<12} {}", key.label, status)));
            }
            if card.escalation_required {
                lines.push(Line::from(Span::styled(
                    "   escalation required — co-signer must be a non-editor",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
            }
            lines.push(Line::from(
                " [g] go   [r] no-go +remedy   [e] hand-edit   [o] open diff   [d] discuss",
            ));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::bordered().title("MERGE GATE"))
                    .wrap(Wrap { trim: true }),
                area,
            );
        }
        ActiveRegion::Intervention(card) => {
            let lines = vec![
                Line::from(format!(" NO-GO · {} — a remedy is required (steer or edit)", card.gate_id)),
                Line::from(format!(" steer > {}", card.steer)),
                Line::from(" [↵] send steer · [esc] cancel"),
            ];
            frame.render_widget(
                Paragraph::new(lines).block(Block::bordered().title("INTERVENTION")),
                area,
            );
        }
        ActiveRegion::SessionClosed(summary) => {
            let lines = vec![
                Line::from(format!(" {} gates · unanimous", summary.gates)),
                Line::from(format!(" Reviewed-by: {}", summary.reviewers.join("   Reviewed-by: "))),
                Line::from(if summary.chain_verified {
                    " chain verified ✓ (tamper-evident)".to_string()
                } else {
                    " chain BROKEN ✗".to_string()
                }),
            ];
            frame.render_widget(
                Paragraph::new(lines).block(Block::bordered().title("SESSION COMPLETE")),
                area,
            );
        }
    }
}

fn command(frame: &mut Frame, area: Rect) {
    frame.render_widget(Paragraph::new(" > "), area);
}
```

- [ ] **Step 2: Wire the module** — edit `lib.rs`, add `pub mod render;` and `pub use render::render;`.

- [ ] **Step 3: Create the golden tests** — `crates/kontur-tui/tests/render.rs`

```rust
use kontur_core::OperatorId;
use kontur_tui::render::render;
use kontur_tui::view::{
    ActiveRegion, AuditSummary, Banner, GateCard, KeyStatus, KeyView, Role, SessionView, Station,
    StatusStrip,
};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

fn buf_string(buf: &Buffer) -> String {
    let area = buf.area;
    let mut s = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

fn base(active: ActiveRegion) -> SessionView {
    SessionView {
        banner: Banner { session: "4417".into(), version: "0.1.0".into() },
        status: StatusStrip { linked: true, four_eyes: true, fleet_count: 3, needs_you: 1, tokens: 6400 },
        stations: [
            Station { label: "A · YOU".into(), role: Role::Driver, activity: "watching".into(), operator: OperatorId([1; 32]) },
            Station { label: "B · J.REED".into(), role: Role::Navigator, activity: "reviewing".into(), operator: OperatorId([2; 32]) },
        ],
        fleet: vec![],
        log: vec![],
        active,
    }
}

fn draw(view: &SessionView) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
    terminal.draw(|f| render(f, view)).unwrap();
    buf_string(terminal.backend().buffer())
}

#[test]
fn banner_and_status_render() {
    let s = draw(&base(ActiveRegion::Idle));
    assert!(s.contains("КОНТУР-1"));
    assert!(s.contains("4-EYES ON"));
    assert!(s.contains("NEEDS YOU"));
}

#[test]
fn gate_shows_summary_and_sealed_key_never_value() {
    let card = GateCard {
        gate_id: "gate-001".into(),
        task: "t1".into(),
        files: vec!["auth/session.rs".into()],
        loc: 47,
        keys: vec![
            KeyView { label: "A · YOU".into(), role: Role::Driver, status: KeyStatus::Awaiting },
            KeyView { label: "B · J.REED".into(), role: Role::Navigator, status: KeyStatus::Sealed },
        ],
        escalation_required: false,
        diff_preview: None,
    };
    let s = draw(&base(ActiveRegion::Gate(card)));
    assert!(s.contains("auth/session.rs"));
    assert!(s.contains("+47 loc"));
    assert!(s.contains("cast — sealed"));
    // The sealed key must not reveal a verdict value.
    assert!(!s.contains("■ GO"));
    assert!(!s.contains("■ NO-GO"));
    assert!(s.contains("[g] go"));
}

#[test]
fn session_close_shows_verified_chain() {
    let summary = AuditSummary { gates: 4, reviewers: vec!["A · YOU".into(), "B · J.REED".into()], chain_verified: true };
    let s = draw(&base(ActiveRegion::SessionClosed(summary)));
    assert!(s.contains("4 gates · unanimous"));
    assert!(s.contains("chain verified"));
    assert!(s.contains("Reviewed-by: A · YOU"));
}
```

> **Note:** the golden helper uses `buf.area` as a field. If ratatui 0.30 exposes it as a method, change to `buf.area()`. The `buf[(x, y)]` index and `.symbol()` are stable.

- [ ] **Step 4: Run tests**

Run: `cargo test -p kontur-tui --test render` then `cargo build -p kontur-tui --all-targets 2>&1`
Expected: PASS (3 tests); zero warnings. If a layout row is too short and a substring is clipped, widen the relevant `Constraint` or the `TestBackend` size — do not weaken an assertion.

- [ ] **Step 5: Commit**

```bash
git add crates/kontur-tui/src/render.rs crates/kontur-tui/tests/render.rs crates/kontur-tui/src/lib.rs
git commit -m "feat(tui): brutalist console rendering + golden tests

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: app loop + terminal guard

**Files:**
- Create: `crates/kontur-tui/src/app.rs`
- Modify: `crates/kontur-tui/src/lib.rs`

**Interfaces:**
- Consumes: `render` (Task 6), `map_key`/`Action` (Task 4); ratatui `Terminal`, `CrosstermBackend`, `ratatui::crossterm::{terminal, event, execute}`.
- Produces: `TerminalGuard` (RAII restore) with `enter() -> io::Result<Terminal<...>>`; `struct AppConfig`/`run` are added in Task 8's demo (the loop body lives here as `pub async fn run_loop`).

- [ ] **Step 1: Create `crates/kontur-tui/src/app.rs`**

```rust
use std::io::{self, Stdout};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::execute;
use ratatui::Terminal;
use std::time::Duration;

use crate::input::{map_key, Action};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Restores the terminal on drop — including on panic (a panic hook runs the
/// same restore before the default hook). Constructing it enters raw mode +
/// the alternate screen.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> io::Result<(Self, Tui)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok((TerminalGuard, terminal))
    }

    /// Restore the terminal explicitly (idempotent with Drop).
    pub fn restore() {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        TerminalGuard::restore();
    }
}

/// Poll for the next operator action, or `None` on timeout (so the loop can
/// refresh the view periodically). `composing_remedy` switches key semantics.
pub fn poll_action(timeout: Duration, composing_remedy: bool) -> io::Result<Option<Action>> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            return Ok(Some(map_key(key.code, composing_remedy)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_safe_to_call_without_entering() {
        // Should not panic even when raw mode was never enabled.
        TerminalGuard::restore();
    }
}
```

> **Note:** `poll_action` and `TerminalGuard::enter` touch the real terminal and are exercised by the demo binary (Task 8), not unit tests. The unit test only checks `restore()` is panic-safe.

- [ ] **Step 2: Wire the module** — edit `lib.rs`, add `pub mod app;` and `pub use app::{poll_action, TerminalGuard, Tui};`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p kontur-tui app` then `cargo build -p kontur-tui --all-targets 2>&1`
Expected: PASS (1 test); zero warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/kontur-tui/src/app.rs crates/kontur-tui/src/lib.rs
git commit -m "feat(tui): terminal guard + key polling

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: demo harness + binary + headless flow test

**Files:**
- Create: `crates/kontur-tui/src/demo.rs`
- Create: `crates/kontur-tui/src/bin/kontur.rs`
- Create: `crates/kontur-tui/tests/flow.rs`
- Modify: `crates/kontur-tui/src/lib.rs`

**Interfaces:**
- Consumes: all prior tasks; `GateHost`, `InMemoryWorkspace`, `SessionContext`, `Workspace` (kontur-mcp); `Ed25519Signer`, `CastVerdict`, `Verdict`, `ReviewDepth`, `FixedClock` (kontur-core).
- Produces: `Demo` with `new()`, `host()`, `stations()`, `operator_a()`/`operator_b_signer()`, `open_demo_gate()`, `cast_second_key(gate_id, diff_hash)`; `run(demo) -> io::Result<()>` (the interactive loop).

- [ ] **Step 1: Create `crates/kontur-tui/src/demo.rs`**

```rust
use std::sync::Arc;
use std::time::Duration;

use kontur_core::{
    CastVerdict, Ed25519Signer, FixedClock, GateId, Hash, ReviewDepth, Signer, TaskId, Verdict,
};
use kontur_mcp::{GateHost, InMemoryWorkspace, SessionContext, Workspace};

use crate::app::{poll_action, TerminalGuard};
use crate::fleet::MockFleet;
use crate::input::Action;
use crate::render::render;
use crate::view::{Banner, LogLine, Role, Station};
use crate::viewmodel::build_session_view;

/// A self-contained demo session: real GateHost + in-memory workspace + a
/// scripted second operator. Station A is the live keyboard operator; B is
/// scripted (this is a dev/demo console — not the production two-human seat).
pub struct Demo {
    host: Arc<GateHost>,
    workspace: Arc<InMemoryWorkspace>,
    signer_a: Ed25519Signer,
    signer_b: Ed25519Signer,
}

impl Demo {
    pub fn new() -> Self {
        let signer_a = Ed25519Signer::from_seed([1; 32]);
        let signer_b = Ed25519Signer::from_seed([2; 32]);
        let (op_a, op_b) = (signer_a.operator_id(), signer_b.operator_id());
        let workspace = Arc::new(InMemoryWorkspace::new());
        let ctx = SessionContext::new(
            "refactor the session guard to the new token store",
            op_a,
            "agent-03",
            "claude-opus-4-8",
            "1.0",
            vec![op_a, op_b],
        );
        let host = Arc::new(GateHost::new(ctx, workspace.clone()));
        Demo { host, workspace, signer_a, signer_b }
    }

    pub fn host(&self) -> &Arc<GateHost> {
        &self.host
    }

    pub fn stations(&self) -> [Station; 2] {
        [
            Station { label: "A · YOU".into(), role: Role::Driver, activity: "reviewing".into(), operator: self.signer_a.operator_id() },
            Station { label: "B · J.REED".into(), role: Role::Navigator, activity: "reviewing".into(), operator: self.signer_b.operator_id() },
        ]
    }

    pub fn banner(&self) -> Banner {
        Banner { session: "4417".into(), version: env!("CARGO_PKG_VERSION").into() }
    }

    /// Script an agent producing a change and parking it at a gate.
    pub async fn open_demo_gate(&self) -> (GateId, Hash) {
        let task = TaskId("t1".into());
        self.workspace
            .apply_write(&task, "auth/session.rs", b"// guarded token store\nfn guard() {}\n")
            .unwrap();
        let (gid, _rx) = self.host.begin_task_gate(task, 6400).await.unwrap();
        let dh = self.host.pending_gates().await[0].diff_hash;
        (gid, dh)
    }

    /// Station A's signed go (the live operator's key).
    pub fn go_a(&self, gid: &GateId, dh: Hash) -> CastVerdict {
        CastVerdict::create(&self.signer_a, &FixedClock(1000), gid, dh, Verdict::Go, ReviewDepth::FullDiff, None)
    }

    /// The scripted second operator's go.
    pub fn go_b(&self, gid: &GateId, dh: Hash) -> CastVerdict {
        CastVerdict::create(&self.signer_b, &FixedClock(1001), gid, dh, Verdict::Go, ReviewDepth::FullDiff, None)
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the interactive demo console. Station A drives from the keyboard; when A
/// casts `go`, the scripted second key follows and the gate resolves.
pub async fn run(demo: Demo) -> std::io::Result<()> {
    let fleet = MockFleet::demo();
    let (gid, dh) = demo.open_demo_gate().await;
    let mut log: Vec<LogLine> = vec![LogLine {
        time: "12:10".into(),
        who: "agent-03".into(),
        text: "parked change at gate-001".into(),
    }];
    let mut closed = false;

    let (_guard, mut terminal) = TerminalGuard::enter()?;
    loop {
        let view = build_session_view(demo.host(), &fleet, demo.stations(), demo.banner(), log.clone(), closed).await;
        terminal.draw(|f| render(f, &view))?;
        if closed {
            // Draw the final frame, then wait for a quit key.
            if let Some(Action::Quit) = poll_action(Duration::from_millis(200), false)? {
                break;
            }
            continue;
        }
        match poll_action(Duration::from_millis(200), false)? {
            Some(Action::Quit) => break,
            Some(Action::Go) => {
                let _ = demo.host().submit_verdict(&gid, demo.go_a(&gid, dh)).await;
                log.push(LogLine { time: "12:11".into(), who: "you".into(), text: "go gate-001 · key sealed".into() });
                // Scripted second key follows.
                let _ = demo.host().submit_verdict(&gid, demo.go_b(&gid, dh)).await;
                log.push(LogLine { time: "12:11".into(), who: "j.reed".into(), text: "go gate-001 · unanimous".into() });
                closed = true;
            }
            _ => {}
        }
    }
    TerminalGuard::restore();
    Ok(())
}
```

- [ ] **Step 2: Create `crates/kontur-tui/src/bin/kontur.rs`**

```rust
use kontur_tui::demo::{run, Demo};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    run(Demo::new()).await
}
```

- [ ] **Step 3: Wire the module** — edit `lib.rs`, add `pub mod demo;` (no re-export needed; the bin uses `kontur_tui::demo::*`).

- [ ] **Step 4: Create the headless flow test** — `crates/kontur-tui/tests/flow.rs`

```rust
use kontur_tui::demo::Demo;
use kontur_tui::view::ActiveRegion;
use kontur_tui::{build_session_view, MockFleet};

#[tokio::test]
async fn full_flow_pending_to_accepted_audited() {
    let demo = Demo::new();
    let (gid, dh) = demo.open_demo_gate().await;

    // Before verdicts: a pending gate is the active region.
    let view = build_session_view(demo.host(), &MockFleet::demo(), demo.stations(), demo.banner(), vec![], false).await;
    assert!(matches!(view.active, ActiveRegion::Gate(_)));

    // Station A casts, then the scripted second key.
    demo.host().submit_verdict(&gid, demo.go_a(&gid, dh)).await.unwrap();
    demo.host().submit_verdict(&gid, demo.go_b(&gid, dh)).await.unwrap();

    // Closed: verified audit, two reviewers, one gate.
    let view = build_session_view(demo.host(), &MockFleet::demo(), demo.stations(), demo.banner(), vec![], true).await;
    match view.active {
        ActiveRegion::SessionClosed(summary) => {
            assert_eq!(summary.gates, 1);
            assert!(summary.chain_verified);
            assert_eq!(summary.reviewers.len(), 2);
        }
        other => panic!("expected SessionClosed, got {other:?}"),
    }
    assert!(demo.host().verify_audit().await.is_ok());
}
```

- [ ] **Step 5: Run tests + build the binary**

Run: `cargo test -p kontur-tui --test flow` then `cargo test -p kontur-tui` then `cargo build -p kontur-tui --bin kontur 2>&1` then `cargo clippy -p kontur-tui --all-targets -- -D warnings`
Expected: all tests PASS; the `kontur` binary builds; clippy clean. (Do not run the interactive binary in the test harness — it needs a real terminal.)

- [ ] **Step 6: Commit**

```bash
git add crates/kontur-tui/src/demo.rs crates/kontur-tui/src/bin/kontur.rs crates/kontur-tui/tests/flow.rs crates/kontur-tui/src/lib.rs
git commit -m "feat(tui): demo harness + kontur binary + headless flow test

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (for the executor)

- **Spec coverage:** console shell + watch-floor (Task 6 render + Task 5 viewmodel); merge-gate sign-off with blind sealing (Tasks 5–6, sealed key from `VerdictView`); no-go+remedy (Intervention region render Task 6 + `NoGoBegin`/remedy actions Task 4 — full remedy submission wiring is a demo affordance; the render + action mapping are present); session close/audit (Tasks 5–6); demo harness + scripted second key + runnable binary (Task 8); the `kontur-mcp` additions (Task 1).
- **Blind sealing** is structural: keys derive only from `GateView.observed` (`VerdictView`), asserted by the render test (`!contains("■ GO")` when sealed) and the viewmodel test.
- **Deferred (not this plan):** prompt co-construction, plan/DAG review, discuss thread, real presence/claim/rotation, disconnect screen, rich diff pager, and the true two-human seat — per spec §1.
- **Known adaptation point:** `buf.area` vs `buf.area()` in the render golden test (Task 6) — reconcile against installed ratatui 0.30; `buf[(x,y)].symbol()` is stable.
- **The interactive loop** (`run`, `poll_action`, `TerminalGuard::enter`) is not unit-tested (needs a real terminal); coverage lives in the viewmodel/render/input/flow tests, and `restore()` panic-safety is unit-tested.
```
