use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::canonical::{canonical_bytes, sha256};
use crate::hold::{DualHold, HoldState};
use crate::ids::{GateId, Hash, OperatorId, Sig, TaskId, Timestamp};
use crate::policy::{Authorship, Outcome};
use crate::verdict::{ReviewDepth, Verdict};

/// The fixed gate id for the dispatch gate. The dispatch approval's signature
/// binds to this id (and to `prompt_hash`), so a dispatch `go` can never be
/// replayed onto a merge gate and vice-versa.
pub fn dispatch_gate_id() -> GateId {
    GateId("dispatch".into())
}

/// The content an operator signs when approving a dispatch: the SHA-256 of the
/// exact prompt bytes. This occupies the `diff_hash` slot of the reused
/// signing primitive — for a dispatch there is no diff, the prompt *is* the
/// content under review. Editing the prompt changes this hash, so a stale
/// approval signature can never count (crypto-enforced anchoring).
pub fn prompt_hash(prompt: &str) -> Hash {
    sha256(prompt.as_bytes())
}

/// Provenance of the change (PRD §9). These fields originate upstream (prompt
/// co-construction, the agent adapter) and are supplied by the caller.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub task_id: TaskId,
    pub prompt: String,
    pub prompt_author: OperatorId,
    pub agent_id: String,
    pub agent_model: String,
    pub agent_version: String,
    pub diff_hash: Hash,
    pub files: Vec<String>,
    pub loc: u32,
    pub tokens: u64,
}

/// One checker's signed decision, as recorded.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CheckerEntry {
    pub operator: OperatorId,
    pub cast_at: Timestamp,
    pub verdict: Verdict,
    pub depth: ReviewDepth,
    pub comment: Option<String>,
    pub signature: Sig,
}

/// Everything in a gate record except its own hash — the bytes that get hashed.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RecordCore {
    pub prev_hash: Hash,
    pub gate_id: GateId,
    pub provenance: Provenance,
    pub authorship: Authorship,
    pub checkers: Vec<CheckerEntry>,
    pub outcome: Outcome,
}

/// A signed, hash-chained gate record (PRD §9). Immutable once built.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GateRecord {
    pub core: RecordCore,
    pub this_hash: Hash,
}

#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum RecordError {
    #[error("cannot record an unresolved gate")]
    HoldUnresolved,
}

impl GateRecord {
    /// Build the record for a resolved hold, chained to `prev_hash`. A hold is
    /// resolved when it is either `Satisfied` (two go verdicts, merge path) or
    /// `Blocked` (a no-go resolved it, intervention path). A blocked record's
    /// checker entries already carry the `NoGo(Remedy)` verdict and its
    /// signature — dissent and remedy are chained with no new fields. Open or
    /// in-progress holds are rejected.
    pub fn build(
        prev_hash: Hash,
        provenance: Provenance,
        hold: &DualHold,
    ) -> Result<GateRecord, RecordError> {
        let outcome = match hold.state() {
            HoldState::Satisfied => hold.outcome().expect("satisfied hold has an outcome"),
            HoldState::Blocked => Outcome::Blocked,
            _ => return Err(RecordError::HoldUnresolved),
        };

        let checkers = checker_entries(hold);

        let core = RecordCore {
            prev_hash,
            gate_id: hold.gate_id().clone(),
            provenance,
            authorship: hold.authorship(),
            checkers,
            outcome,
        };
        let this_hash = sha256(&canonical_bytes(&core));
        Ok(GateRecord { core, this_hash })
    }

    /// Recompute the hash from the core — used by chain verification.
    pub fn recompute_hash(&self) -> Hash {
        sha256(&canonical_bytes(&self.core))
    }
}

/// Extract the signed verdicts from a resolved hold as recordable checker
/// entries. Shared by the merge-gate and dispatch-gate record builders.
pub(crate) fn checker_entries(hold: &DualHold) -> Vec<CheckerEntry> {
    hold.raw_verdicts()
        .iter()
        .map(|sv| {
            let cv = sv.raw();
            CheckerEntry {
                operator: cv.operator,
                cast_at: cv.cast_at,
                verdict: cv.verdict.clone(),
                depth: cv.depth,
                comment: cv.comment.clone(),
                signature: cv.signature,
            }
        })
        .collect()
}

/// How a dispatch gate concluded.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DispatchOutcome {
    /// Both operators signed `go`; the prompt was handed to the agent.
    Dispatched,
    /// The compose was killed/closed before dispatch. Carries whatever signed
    /// approvals had been gathered (0..=2); integrity rests on the hash chain
    /// and any signatures present. Nothing reached the agent.
    Abandoned,
}

/// The bytes of a dispatch record that get hashed. Unlike a merge record this
/// is prompt-centric, not diff-centric: the prompt is a first-class field, not
/// smuggled into a diff slot.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DispatchCore {
    pub prev_hash: Hash,
    pub gate_id: GateId,
    pub prompt: String,
    pub prompt_author: OperatorId,
    /// The operators who signed `go` on this prompt (0..=2). Each signature
    /// binds to `prompt_hash(prompt)`, verified in `verify_chain`.
    pub approvers: Vec<CheckerEntry>,
    pub outcome: DispatchOutcome,
    /// The latest approver's cast time, or `Timestamp(0)` when abandoned with no
    /// approvals. A convenience summary; the per-approver `cast_at` is authoritative.
    pub resolved_at: Timestamp,
}

/// A signed, hash-chained dispatch-gate record. Immutable once built.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DispatchRecord {
    pub core: DispatchCore,
    pub this_hash: Hash,
}

impl DispatchRecord {
    /// Build the record for a dispatched prompt: the hold must be `Satisfied`
    /// (two distinct signed `go` verdicts — independence enforced by the hold).
    pub fn build_dispatched(
        prev_hash: Hash,
        prompt: String,
        prompt_author: OperatorId,
        hold: &DualHold,
    ) -> Result<DispatchRecord, RecordError> {
        if hold.state() != HoldState::Satisfied {
            return Err(RecordError::HoldUnresolved);
        }
        let approvers = checker_entries(hold);
        Ok(Self::assemble(
            prev_hash,
            hold.gate_id().clone(),
            prompt,
            prompt_author,
            approvers,
            DispatchOutcome::Dispatched,
        ))
    }

    /// Build the record for an abandoned compose: whatever signed approvals were
    /// gathered (0..=2), no resolved hold required. Nothing merged.
    pub fn build_abandoned(
        prev_hash: Hash,
        prompt: String,
        prompt_author: OperatorId,
        approvers: Vec<CheckerEntry>,
    ) -> DispatchRecord {
        Self::assemble(
            prev_hash,
            dispatch_gate_id(),
            prompt,
            prompt_author,
            approvers,
            DispatchOutcome::Abandoned,
        )
    }

    fn assemble(
        prev_hash: Hash,
        gate_id: GateId,
        prompt: String,
        prompt_author: OperatorId,
        approvers: Vec<CheckerEntry>,
        outcome: DispatchOutcome,
    ) -> DispatchRecord {
        let resolved_at = approvers
            .iter()
            .map(|a| a.cast_at)
            .max()
            .unwrap_or(Timestamp(0));
        let core = DispatchCore {
            prev_hash,
            gate_id,
            prompt,
            prompt_author,
            approvers,
            outcome,
            resolved_at,
        };
        let this_hash = sha256(&canonical_bytes(&core));
        DispatchRecord { core, this_hash }
    }

    /// Recompute the hash from the core — used by chain verification.
    pub fn recompute_hash(&self) -> Hash {
        sha256(&canonical_bytes(&self.core))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eligibility::MakerSet;
    use crate::ids::TaskId;
    use crate::sign::{Ed25519Signer, FixedClock, Signer};
    use crate::verdict::CastVerdict;
    use crate::{GatePolicy, ReviewDepth};

    fn satisfied_hold() -> DualHold {
        let mut h = DualHold::new(
            GateId("g1".into()),
            TaskId("t1".into()),
            Hash([9u8; 32]),
            GatePolicy::default(),
            MakerSet::new(),
            Authorship::Agent,
        );
        for seed in [1u8, 2u8] {
            let signer = Ed25519Signer::from_seed([seed; 32]);
            let clock = FixedClock(1000 + seed as i64);
            let cv = CastVerdict::create(
                &signer,
                &clock,
                h.gate_id(),
                h.diff_hash(),
                Verdict::Go,
                ReviewDepth::FullDiff,
                None,
            );
            let ev = h.version();
            h.cast(ev, cv).unwrap();
        }
        h
    }

    fn provenance() -> Provenance {
        Provenance {
            task_id: TaskId("t1".into()),
            prompt: "refactor session guard".into(),
            prompt_author: Ed25519Signer::from_seed([1; 32]).operator_id(),
            agent_id: "agent-03".into(),
            agent_model: "claude-opus-4-8".into(),
            agent_version: "1.0".into(),
            diff_hash: Hash([9u8; 32]),
            files: vec!["auth/session.ts".into()],
            loc: 59,
            tokens: 6400,
        }
    }

    #[test]
    fn build_records_two_checkers_and_hashes() {
        let h = satisfied_hold();
        let rec = GateRecord::build(Hash([0u8; 32]), provenance(), &h).unwrap();
        assert_eq!(rec.core.checkers.len(), 2);
        assert_eq!(rec.core.outcome, Outcome::Unanimous);
        assert_eq!(rec.this_hash, rec.recompute_hash());
    }

    #[test]
    fn refuses_unsatisfied_hold() {
        // A fresh, open hold — never satisfied.
        let h = DualHold::new(
            GateId("g2".into()),
            TaskId("t2".into()),
            Hash([1u8; 32]),
            GatePolicy::default(),
            MakerSet::new(),
            Authorship::Agent,
        );
        let err = GateRecord::build(Hash([0u8; 32]), provenance(), &h).unwrap_err();
        assert_eq!(err, RecordError::HoldUnresolved);
    }

    #[test]
    fn dispatched_record_refuses_unsatisfied_hold() {
        // A partial hold (one key) cannot back a Dispatched record.
        let mut h = DualHold::new(
            dispatch_gate_id(),
            TaskId("dispatch".into()),
            prompt_hash("ship it"),
            GatePolicy {
                blind: false,
                ..GatePolicy::default()
            },
            MakerSet::new(),
            Authorship::HandEdited,
        );
        let signer = Ed25519Signer::from_seed([1; 32]);
        let cv = CastVerdict::create(
            &signer,
            &FixedClock(1000),
            h.gate_id(),
            h.diff_hash(),
            Verdict::Go,
            ReviewDepth::FullDiff,
            None,
        );
        h.cast(0, cv).unwrap();
        let author = signer.operator_id();
        let err = DispatchRecord::build_dispatched(Hash([0u8; 32]), "ship it".into(), author, &h)
            .unwrap_err();
        assert_eq!(err, RecordError::HoldUnresolved);
    }

    #[test]
    fn abandoned_record_keeps_partial_approvals() {
        // One operator signed before the compose was killed: the abandoned
        // record carries that single approval and stamps resolved_at from it.
        let signer = Ed25519Signer::from_seed([1; 32]);
        let cv = CastVerdict::create(
            &signer,
            &FixedClock(1234),
            &dispatch_gate_id(),
            prompt_hash("half done"),
            Verdict::Go,
            ReviewDepth::FullDiff,
            None,
        );
        let approver = CheckerEntry {
            operator: cv.operator,
            cast_at: cv.cast_at,
            verdict: cv.verdict.clone(),
            depth: cv.depth,
            comment: cv.comment.clone(),
            signature: cv.signature,
        };
        let rec = DispatchRecord::build_abandoned(
            Hash([0u8; 32]),
            "half done".into(),
            signer.operator_id(),
            vec![approver],
        );
        assert_eq!(rec.core.outcome, DispatchOutcome::Abandoned);
        assert_eq!(rec.core.approvers.len(), 1);
        assert_eq!(rec.core.resolved_at, Timestamp(1234));
        assert_eq!(rec.this_hash, rec.recompute_hash());
    }

    fn blocked_hold() -> DualHold {
        let mut h = DualHold::new(
            GateId("g1".into()),
            TaskId("t1".into()),
            Hash([9u8; 32]),
            GatePolicy::default(),
            MakerSet::new(),
            Authorship::Agent,
        );
        let s1 = Ed25519Signer::from_seed([1; 32]);
        let cv = CastVerdict::create(
            &s1,
            &FixedClock(1000),
            h.gate_id(),
            h.diff_hash(),
            Verdict::Go,
            ReviewDepth::FullDiff,
            None,
        );
        h.cast(0, cv).unwrap();
        let s2 = Ed25519Signer::from_seed([2; 32]);
        let cv = CastVerdict::create(
            &s2,
            &FixedClock(1001),
            h.gate_id(),
            h.diff_hash(),
            Verdict::NoGo(crate::Remedy::Steer("cache it".into())),
            ReviewDepth::FullDiff,
            None,
        );
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
        let h = DualHold::new(
            GateId("g2".into()),
            TaskId("t2".into()),
            Hash([1u8; 32]),
            GatePolicy::default(),
            MakerSet::new(),
            Authorship::Agent,
        );
        assert_eq!(
            GateRecord::build(Hash([0u8; 32]), provenance(), &h).unwrap_err(),
            RecordError::HoldUnresolved
        );
    }
}
