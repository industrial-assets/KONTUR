# Design — Sign & audit the dispatch gate

**Date:** 2026-07-23
**Status:** approved (pending spec review)
**Area:** four-eyes engine (`kontur-core`), enforcement plane (`kontur-mcp`), session server/client (`kontur-net`), console (`kontur-tui`), `kontur audit` CLI

## Problem

The dispatched prompt is already embedded in every **merge-gate** audit record
(`Provenance.prompt`, populated from `SessionContext.prompt`). But the **dispatch
gate itself** — the point where both operators approve the composed prompt
(`[p]` compose → `[g]` consent) — emits **no** audit record. It resolves via bare
`ready` flags (`ClientMsg::Ready`) with no signing.

Consequences:

- If a dispatch is approved but the agent never lands a completed task (session
  abandoned, agent fails, killed before any `propose_task_complete`), there is
  **no signed record of what was dispatched and who approved it**.
- This conflicts with invariant #6 ("every gate emits a signed record,
  hash-chained to the previous one") — the dispatch gate is a gate.

## Goal

Emit a **signed, hash-chained `DispatchRecord`** into the same audit chain when
the dispatch gate resolves, so *what was dispatched and who approved it* is
durably recorded — including the case where the compose is abandoned before
dispatch. Make the dispatch `[g]` a **signed cast over the prompt hash** so
"both approvers, signed" is cryptographically real, not cosmetic.

Non-goals: no change to merge-gate provenance (prompt stays there too); no blind
review at dispatch (open co-composition); no change to merge-gate mechanics; the
two-key merge-to-main guarantee is untouched (nothing here merges).

## Key facts grounding the design

- Signing is **client-side**: `CastVerdict::create(&signer, &clock, gate_id,
  diff_hash, verdict, depth, comment)` signs
  `SignedContent { gate_id, diff_hash, operator, verdict, depth, cast_at }`;
  `verify_signature(gate_id, diff_hash)` re-derives and verifies. The key never
  leaves the client. (`crates/kontur-core/src/verdict.rs`)
- `DualHold` already enforces two-distinct-operator independence and verifies
  each cast's signature. (`crates/kontur-core/src/hold.rs`)
- The audit chain is an in-memory `Vec<GateRecord>` owned by the gate host,
  persisted at session close — committed into the reviewed merge commit under
  `.kontur/` on merge, written as a loose `.kontur/audit-<head>.json` on abandon.
  (`crates/kontur-mcp/src/gatehost.rs`)
- Dispatch resolves today by both seats toggling `ready`; on both-ready in
  `Phase::DispatchReady` the server locks the prompt via `host.set_prompt` and
  advances to `Phase::PlanReview`. (`crates/kontur-net/src/server.rs`)

## Design decisions (resolved during brainstorming)

1. **Record shape: first-class `DispatchRecord`** (not a reused `GateRecord`).
   The prompt is an explicit field, not smuggled into a diff slot — honest per
   the brutalist principle, and dispatch vs merge records stay distinguishable.
2. **Signed consent at dispatch: yes.** `[g]` becomes a signed cast over
   `sha256(prompt)`. Editing the prompt changes the hash and invalidates a prior
   signature — the existing "edit resets both" anchoring, now crypto-enforced.
3. **Signature binding: reuse `CastVerdict`** with `diff_hash := sha256(prompt)`,
   `verdict = Go`, sentinel `depth`. Zero new crypto surface; reuses the audited
   signing + `DualHold` independence path. The *record* stays honest (explicit
   `prompt` field); the prompt-in-diff_hash is an internal signing detail only.
4. **Record abandoned dispatches too.** A `DispatchRecord` may carry outcome
   `Abandoned` with 0–2 signed approvers, capturing a compose that never
   dispatched.

## Architecture

### 1. `kontur-core` — new record kind

In `audit/record.rs`:

```rust
pub enum DispatchOutcome { Dispatched, Abandoned }

pub struct DispatchCore {
    pub prev_hash: Hash,
    pub gate_id: GateId,
    pub prompt: String,
    pub prompt_author: OperatorId,
    pub approvers: Vec<CheckerEntry>, // 0..=2; reuses existing CheckerEntry
    pub outcome: DispatchOutcome,
    pub resolved_at: Timestamp,
}

pub struct DispatchRecord {
    pub core: DispatchCore,
    pub this_hash: Hash, // sha256(canonical_bytes(&core))
}
```

- `approvers` reuses `CheckerEntry` (operator, cast_at, verdict=`Go`, depth,
  comment, **signature**) — no new signed type.
- `DispatchRecord::build_dispatched(prev_hash, prompt, prompt_author, &hold)` —
  from a resolved (`Satisfied`) `DualHold`, pulls the two signed `Go` verdicts as
  approvers, outcome `Dispatched`.
- `DispatchRecord::build_abandoned(prev_hash, prompt, prompt_author, approvers,
  resolved_at)` — from whatever signed approvals exist (0–2), outcome
  `Abandoned`; does **not** require a resolved hold.
- Helper `prompt_hash(prompt: &str) -> Hash` = `sha256(prompt.as_bytes())`.

**Chain becomes an enum.** `AuditChain` holds `Vec<AuditEntry>`:

```rust
#[serde(tag = "kind")]
pub enum AuditEntry {
    #[serde(rename = "merge")]    Merge(GateRecord),
    #[serde(rename = "dispatch")] Dispatch(DispatchRecord),
}
```

- `head()`, `append()` (prev_hash == head check), and `verify_chain()` operate
  uniformly on each entry's `prev_hash` / `this_hash` / recomputed hash.
- `verify_chain()` verifies each dispatch approver's signature against
  `prompt_hash(core.prompt)` exactly as it verifies merge checker signatures
  against `diff_hash`. Abandoned records with zero approvers verify on the hash
  chain alone.

### 2. `kontur-mcp` (`gatehost.rs`) — owns the chain

- `record_dispatch(prompt, prompt_author, [cast_a, cast_b]) -> Hash`: builds a
  **non-blind**, `Authorship::Human` `DualHold` over `prompt_hash(prompt)`, casts
  both signed verdicts (reusing existing cast/verify → independence + signatures
  enforced), and on `Satisfied` builds a `Dispatched` `DispatchRecord`, appends
  it, returns the new head. Mirrors `submit_verdict`'s single-place responsibility.
- `record_dispatch_abandoned(prompt, prompt_author, approvers) -> Option<Hash>`:
  appends an `Abandoned` record. No-op (returns `None`) when `prompt` is empty
  **and** `approvers` is empty (noise guard).
- The gate host exposes the **current dispatch prompt hash** so the server can
  validate incoming approvals against the authoritative prompt.

### 3. `kontur-net` — signed consent replaces bare ready

- `PROTOCOL_VERSION` 9 → 10.
- New `ClientMsg::DispatchApprove { verdict: CastVerdict }`. `[g]` at
  `Phase::DispatchReady` sends this (client signs `sha256(current prompt)` via
  its existing signer) **instead of** `ClientMsg::Ready`.
- Server on `DispatchApprove`: verify `verdict.diff_hash == prompt_hash(net.prompt)`,
  `verify_signature` valid, `operator == seat`. Store per-seat approval. When
  **both** seats have a stored approval **both bound to the current prompt hash**:
  call `host.record_dispatch(...)`, then transition to `PlanReview` and
  `host.set_prompt(prompt)` (as today). Reject an approval bound to a stale
  prompt hash. Empty-prompt refusal unchanged.
- **Edit anchoring, crypto-enforced:** any prompt edit (`SetPrompt` / committed
  draft) changes `prompt_hash`, so stored approvals no longer match and are
  cleared — same "edit resets both" rule, but a stale signature cannot count.
- **Abandon path:** when the session is killed (`[K]`) or terminally closed while
  `Phase::DispatchReady` and not yet dispatched, call
  `host.record_dispatch_abandoned(net.prompt, author, collected_approvals)`
  before/within the existing abandon persistence, so the abandoned dispatch is in
  the chain that gets written to the loose audit file.
- `WireSeat`/`WireState`: the dispatch consent indicator reflects "approved
  (signed)" instead of "ready"; cleared on edit.

### 4. `kontur-tui`

- `[g]` in `DispatchReady` → `client.dispatch_approve()` (hashes the current
  authoritative prompt, signs, sends `DispatchApprove`). Other phases unchanged
  (still `ready()`).
- Seat indicator reads **approved** once its signed approval is stored; resets on
  edit. Terse, keyboard-first — no new verbs, same `[g]` key.

### 5. `kontur audit` CLI

- Deserialize `Vec<AuditEntry>`, verify **both** kinds, and print dispatch records
  (prompt, approvers, outcome, timestamp) alongside merge records.
- **Back-compat shim:** attempt `Vec<AuditEntry>`; on deserialize failure fall
  back to `Vec<GateRecord>` and wrap each as `AuditEntry::Merge`, so audit files
  already committed in git history (pre-enum, untagged `{core, this_hash}`) still
  verify.

### 6. Persistence

No new path. `persist_audit` already serializes the in-memory chain (now
`Vec<AuditEntry>`); dispatch records ride along on both the merge-commit and
abandon-file paths. Implementation confirms the abandon path serializes the chain
after the abandoned-dispatch record is appended.

### 7. Docs (same change)

- **PRD** §9 (provenance/audit) and §10.1 (two-signatory mechanism): the dispatch
  gate now emits a signed, hash-chained record; abandoned dispatches recorded.
- **UX doc**: dispatch-gate section — `[g]` is a signed approval; seat shows
  "approved".
- **CLAUDE.md**: status line for the dispatch-gate audit record; note invariant
  #6 is now literally true for the dispatch gate.

## Data flow

```
[compose]  [p] edits prompt --> ClientMsg::SetPrompt / live drafts
                                 server updates net.prompt; clears stored approvals

[consent]  [g] on seat --> client signs sha256(net.prompt) as CastVerdict(Go)
                       --> ClientMsg::DispatchApprove { verdict }
           server: verify hash==current, sig valid, operator==seat; store per-seat

[resolve]  both approvals present & matching prompt hash
           --> host.record_dispatch(prompt, author, [a, b])
               DualHold(prompt_hash) casts both -> Satisfied
               -> DispatchRecord(Dispatched) -> chain.append
           --> phase: DispatchReady -> PlanReview; host.set_prompt(prompt)

[abandon]  [K] / terminal close while DispatchReady & not dispatched
           --> host.record_dispatch_abandoned(prompt, author, collected)
               (skipped if prompt empty AND no approvals)
           --> DispatchRecord(Abandoned) -> chain.append -> persisted to loose file

[close]    persist_audit serializes Vec<AuditEntry>
           merge  -> committed under .kontur/ in the reviewed commit
           abandon-> loose .kontur/audit-<head>.json
```

## Error handling / edge cases

- **Stale approval:** approval whose `diff_hash` != current `prompt_hash` is
  rejected (edit raced the cast). Operator re-approves.
- **Empty prompt:** cannot dispatch (existing refusal); abandoned record skipped
  when empty and no approvals.
- **One-sided approval then abandon:** abandoned record carries the single signed
  approver; verifies on chain + that one signature.
- **Re-approval after edit:** prior stored approval cleared on edit; a fresh cast
  over the new hash is required.
- **Independence:** `Dispatched` requires two distinct operators (enforced by the
  reused `DualHold`). A single operator can never both compose and be the sole
  dispatcher.

## Testing

- **core:** `DispatchRecord` build/hash (both outcomes); mixed-entry chain
  verifies; tampering the prompt breaks `this_hash`; forged / mismatched approver
  signature fails `verify_chain`; `Dispatched` requires two distinct approvers;
  abandoned-with-zero-approvers verifies on chain alone.
- **net:** both-approve → record appended + phase advances; prompt edit clears
  stored approvals; approval bound to a stale prompt hash rejected; empty prompt
  refused; `[K]` at compose emits an `Abandoned` record (and skips when empty +
  no approvals).
- **CLI:** legacy `Vec<GateRecord>` file still verifies via the compat shim; new
  mixed file verifies and prints dispatch records.
- **persistence:** dispatch record survives close on both the merge and abandon
  paths.

## Rollout

Protocol bump (9 → 10) is a breaking wire change; host and operator must be on the
same version (already surfaced via the advisory peer-version footer). No migration
of existing audit files needed — the CLI compat shim reads old files as-is.
