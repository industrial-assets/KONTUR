# Kontur core engine — design spec

**Date:** 2026-07-16 · **Owner:** John · **Status:** Approved for planning

The first buildable slice of Kontur (КОНТУР-1): the **four-eyes core engine** — the
invariant-bearing heart described in PRD §10.1, built as a pure, headless, fully
unit-tested Rust library with no I/O, no network, no TUI, and no live agent.

This is the "part nobody else has built": turning MCP's single-approver gate into an
independent, dual, signed sign-off. If this core is wrong, nothing above it matters.

**Stack (confirmed):** Rust + ratatui for the eventual product; this slice is a pure
library crate (`kontur-core`) with no TUI dependency. Rust chosen for a single static
cross-platform binary (the "easily installed in terminals" requirement) and for making
illegal states unrepresentable in the type system.

---

## 1. Scope

### In scope (this slice)

1. **Dual-hold state machine** — the internals of the `AWAITING_REVIEW` lifecycle state
   (PRD §8, §10.1): `OPEN → PARTIAL → SATISFIED | BLOCKED`. One hold per gated action.
2. **Verdict casting** with **eligibility enforced at cast time, not display** (invariant #2,
   FR-11, §10.1).
3. **Blind sealing** — a cast verdict is unobservable in state, queries, or any read API
   until the hold resolves (invariant #3, FR-12, UX §5.4).
4. **No bare veto** — enforced by the type system: a `NoGo` cannot be constructed without a
   remedy (invariant #4, FR-13).
5. **Hand-edit as a fresh hold** over the combined diff, with an authorship flag and, under
   strict independence, maker exclusion producing an explicit "escalation required" signal
   (invariants #5, FR-15/16/17, §10.1).
6. **Hash-chained, signed audit record** (PRD §9): build, append, and `verify_chain`
   (invariant #6, FR-20). Produces the `Reviewed-by:` trailer data (FR-21) as output.

### Out of scope (later slices, each its own spec)

- Task DAG / plan approval / sequential execution / downstream ripple (FR-6/7/8/18/19) —
  touches the agent adapter.
- MCP hosting and the pause/resume wiring (§10, §10.1 primitive).
- The network attach/sync server: presence, claiming, multi-client state (FR-1/2/3,
  open question #4).
- The two-seat TUI (UX doc in full).
- The real escalation **timer** (this core emits the *signal*; a later crate runs the clock).
- The actual **git merge** and commit-trailer writing (this core emits the *trailer data*).

---

## 2. Non-negotiable invariants this slice owns

Mapped from CLAUDE.md. Each is enforced structurally where possible, not by convention:

| # | Invariant | How this slice enforces it |
|---|-----------|----------------------------|
| 1 | Two independent keys to merge | `DualHold` reaches `SATISFIED` only on two `go` verdicts from two **distinct** operator identities. A second verdict from a present identity is rejected. |
| 2 | Independence at acceptance, not display | Eligibility is checked when a verdict is **cast** (`cast()`), returning an error for an ineligible operator. There is no code path that checks eligibility only at render. |
| 3 | Blind second review | A cast verdict is stored as an opaque `SealedVerdict`; its value is reachable only via `reveal()`, which the hold permits only once resolved. No query, log, or serialization exposes it earlier. |
| 4 | No bare veto | `Verdict::NoGo(Remedy)` — the `NoGo` variant carries a `Remedy` by construction. A veto without a remedy is not representable. |
| 5 | Hand-edit: instant apply, deferred acceptance | Modeled as a **new** `DualHold` over the combined diff, tagged `Authorship::HandEdited`/`Both`, never folded into the agent diff. Under strict independence the editor is excluded and the hold reports `EscalationRequired`. |
| 6 | Tamper-evident audit | Every resolved gate emits a `GateRecord` hash-chained (`prev_hash`) to the previous, each verdict Ed25519-signed. `verify_chain` re-hashes and re-verifies; any mutation fails it. Records are append-only and never mutated. |
| 7 | Fail safe under operator loss | The core never has a code path that clears a hold with a single key. Availability policy (`Park` default / `EscalateAfter`) is data the core reports on; it never degrades to single-key approval. |

---

## 3. Domain model

Illegal states are made unrepresentable wherever the type system allows.

### Identities & keys
- `OperatorId` — a stable identity derived from the operator's Ed25519 public key
  (fingerprint). Equality is what "distinct key" is checked against.

### Verdicts
```
enum Verdict {
    Go,
    NoGo(Remedy),          // invariant #4: a NoGo always carries its fix
}

enum Remedy {
    Steer(String),         // corrective prompt to the agent
    HandEdit(HandEditRef), // reference to a direct human change
}

enum ReviewDepth { FullDiff, Summary, TestsRun }   // captured per §9
```
A `CastVerdict` bundles: `operator: OperatorId`, `verdict: Verdict`, `depth: ReviewDepth`,
optional `comment`, `signature`, and the injected timestamp. When blinding is on, the hold
stores it as an opaque `SealedVerdict` (invariant #3).

### The gate / dual-hold
```
struct GatePolicy {
    required: 2,                    // fixed at 2 for MVP; typed so it can't silently drift
    independence: Independence,     // Strict | Pragmatic
    blind: bool,                    // seal first verdict until both in
    availability: Availability,     // Park | EscalateAfter(Duration)
}

enum Independence { Strict, Pragmatic }
enum Availability { Park, EscalateAfter(Duration) }
enum Authorship  { Agent, HandEdited, Both }

enum HoldState { Open, Partial, Satisfied, Blocked }

struct DualHold {
    gate_id: GateId,
    task_id: TaskId,
    diff_hash: Hash,
    policy: GatePolicy,
    maker: MakerSet,        // prompt author + any hand-editor(s), for eligibility
    authorship: Authorship,
    verdicts: Vec<SealedVerdict>,
    version: u64,           // optimistic-concurrency guard
    state: HoldState,
}
```

### Casting — the single mutating entry point
```
fn cast(&mut self, expected_version: u64, cast: CastVerdict)
    -> Result<HoldOutcome, CastRejected>
```
Rejection reasons (all at cast time, invariant #2):
- `StaleVersion` — `expected_version` != current (concurrent double-cast guard, §10.1).
- `DuplicateIdentity` — this operator already cast on this hold (invariant #1).
- `Ineligible` — strict mode and operator ∈ `maker` set (invariant #2, #5).
- `AlreadyResolved` — hold is `Satisfied`/`Blocked`.

On acceptance:
- First eligible verdict → `Partial` (sealed if `blind`).
- Any `NoGo` → `Blocked`, carrying its `Remedy`; task routes to `INTERVENED` (caller's concern).
- Second `go` from a distinct eligible identity → `Satisfied`; both verdicts revealed together.
- `HoldOutcome` reports whether escalation is required (strict + insufficient eligible
  operators, invariant #7 — signal only, no timer here).

Outcome classification for the record: `Unanimous` vs `ResolvedAfterDisagreement`
(FR-14) — the latter when a discuss/no-go/split preceded the clearing.

---

## 4. Audit chain & crypto

### Record (PRD §9)
`GateRecord` captures: **provenance** (task id + DAG position, verbatim prompt/spec + author,
agent id/model/version, diff hash + files + LOC, agent tool trail, token & cost),
**checks** (per checker: identity, timestamp, verdict, review depth, conditions,
independence assertion), **authorship flag**, **outcome**, **integrity** (`prev_hash`,
`this_hash`, per-verdict signatures).

> Note: provenance fields (prompt text, agent id/model, tool trail, tokens) originate in
> upstream slices. This slice defines the record shape and treats those fields as inputs
> supplied by the caller; it owns the checks/outcome/integrity portions.

### Chain
- `this_hash = SHA-256(canonical_bytes(record_without_this_hash))`; each record carries the
  prior record's `this_hash` as `prev_hash`. A genesis record anchors the session.
- **Canonical serialization**: deterministic CBOR (ciborium, sorted map keys) so bytes —
  and therefore the hash — are stable and independently reproducible.
- `verify_chain(&[GateRecord]) -> Result<(), ChainBreak>` re-hashes every record and
  re-verifies every signature; any byte mutation or broken link fails it.

### Signing
- **Ed25519** (`ed25519-dalek`). Each operator signs their verdict; the signature travels in
  the record's checker entry, giving non-repudiation. `Reviewed-by:` trailer data (FR-21) is
  derived from the verified signatures.

### Determinism (testability + audit reproducibility)
No wall-clock and no RNG inside the core. Injected via traits:
- `Signer` — `sign(bytes) -> Signature`, `public_key()`.
- `Clock` — `now() -> Timestamp`.
- `AuditSink` — `append(record)`, `head() -> Option<Hash>`.

Fakes make every test deterministic.

---

## 5. Rust layout

Workspace from day one so later crates slot in without restructuring.

```
kontur/
  Cargo.toml                    # [workspace]
  crates/
    kontur-core/                # this slice — pure lib, the invariant-bearing heart
      Cargo.toml
      src/
        lib.rs                  # public API surface + re-exports
        verdict.rs              # Verdict, Remedy, ReviewDepth, CastVerdict, SealedVerdict
        policy.rs               # GatePolicy, Independence, Availability, Authorship
        eligibility.rs          # maker-checker eligibility rules (cast-time)
        hold.rs                 # DualHold state machine + cast()
        audit/
          mod.rs
          record.rs             # GateRecord (§9 fields)
          chain.rs              # hash-chain build + verify_chain
          sign.rs               # Signer trait + Ed25519 impl, Clock, AuditSink traits
        error.rs                # CastRejected, ChainBreak, etc. (thiserror)
      tests/
        holds.rs                # integration: full happy/intervene/hand-edit paths
        invariants_prop.rs      # proptest for the invariant table
    # later crates join here: kontur-mcp, kontur-net, kontur-agent, kontur-tui, kontur (bin)
```

**Dependencies** (minimal, well-known — crypto is load-bearing per CLAUDE.md):
`ed25519-dalek`, `sha2`, `ciborium`, `serde`, `thiserror`; `proptest` (dev).

---

## 6. Policy defaults (confirmed)

All three are per-gate `GatePolicy` data; these are the defaults when unspecified:
- **Independence: `Strict`** — the change's maker may not be a checker. `Pragmatic` is the
  opt-in throughput valve. (Resolves PRD open question #2.)
- **Blind: `on`** — first verdict sealed until both are in.
- **Availability: `Park`** — never degrade to one key; escalation is opt-in.

---

## 7. Testing strategy

- **Property tests (`proptest`)** over the invariant table in §2:
  - two distinct `go` ⇒ `Satisfied`; no other input sequence reaches `Satisfied`.
  - a second verdict from a present identity ⇒ `DuplicateIdentity`, state unchanged.
  - any `NoGo` ⇒ `Blocked` and the `Remedy` is retained.
  - a `NoGo` without a remedy is unconstructible (compile-time; asserted by type, not test).
  - a sealed verdict's value is never returned by any read API before `reveal()`.
  - strict mode + maker casting ⇒ `Ineligible`.
  - mutating any byte of any record ⇒ `verify_chain` fails.
  - a stale `expected_version` ⇒ `StaleVersion`, no double-count.
- **Integration tests** for the three narrative paths from UX §7: clean task, caught-in-review
  (`resolved-after-disagreement`), and hand-edit (fresh hold + escalation under strict).
- **Determinism check**: identical inputs + injected clock/keys ⇒ byte-identical records and
  hashes across runs.

---

## 8. Explicit interfaces to later slices (so this core stays decoupled)

- **To the MCP plane:** `DualHold::cast` is called between MCP's pause and resume; `Satisfied`
  ⇒ caller resumes+executes the MCP invocation; `Blocked` ⇒ caller discards and routes to
  `INTERVENED`. The core knows nothing of MCP.
- **To the network/attach server:** the server owns serialization of writes and passes the
  `expected_version` guard; the core is single-writer and rejects stale casts.
- **To the agent adapter:** provenance fields (prompt, agent id/model, tool trail, tokens) are
  inputs to `GateRecord`; the core does not fetch them.
- **To git/merge:** the core emits verified `Reviewed-by:` trailer data and the record chain;
  a later crate writes the commit.
