use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audit::record::{prompt_hash, CheckerEntry, DispatchRecord, GateRecord};
use crate::canonical::canonical_bytes;
use crate::ids::{GateId, Hash, OperatorId};
use crate::sign::verify;
use crate::verdict::{SignedContent, Verdict};

/// The genesis anchor: the `prev_hash` of the first real record.
pub const GENESIS: Hash = Hash([0u8; 32]);

/// One entry in the audit chain. Both kinds share the `prev_hash`/`this_hash`
/// chaining discipline; a merge entry signs over its diff, a dispatch entry
/// over its prompt. Internally tagged so the on-disk JSON is self-describing.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEntry {
    Merge(GateRecord),
    Dispatch(DispatchRecord),
}

impl AuditEntry {
    pub fn prev_hash(&self) -> Hash {
        match self {
            AuditEntry::Merge(r) => r.core.prev_hash,
            AuditEntry::Dispatch(d) => d.core.prev_hash,
        }
    }

    pub fn this_hash(&self) -> Hash {
        match self {
            AuditEntry::Merge(r) => r.this_hash,
            AuditEntry::Dispatch(d) => d.this_hash,
        }
    }

    pub fn recompute_hash(&self) -> Hash {
        match self {
            AuditEntry::Merge(r) => r.recompute_hash(),
            AuditEntry::Dispatch(d) => d.recompute_hash(),
        }
    }

    pub fn gate_id(&self) -> &GateId {
        match self {
            AuditEntry::Merge(r) => &r.core.gate_id,
            AuditEntry::Dispatch(d) => &d.core.gate_id,
        }
    }

    /// The `(gate_id, diff_hash, checker entries)` a verifier needs to check
    /// this entry's signatures. For a dispatch the "diff_hash" is the prompt
    /// hash — the content each approver actually signed.
    fn signed_over(&self) -> (&GateId, Hash, &[CheckerEntry]) {
        match self {
            AuditEntry::Merge(r) => (
                &r.core.gate_id,
                r.core.provenance.diff_hash,
                &r.core.checkers,
            ),
            AuditEntry::Dispatch(d) => (
                &d.core.gate_id,
                prompt_hash(&d.core.prompt),
                &d.core.approvers,
            ),
        }
    }
}

/// An append-only chain of audit entries.
#[derive(Clone, Debug, Default)]
pub struct AuditChain {
    records: Vec<AuditEntry>,
}

#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum ChainError {
    #[error("record's prev_hash does not match the chain head")]
    WrongPrevHash,
}

#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum ChainBreak {
    #[error("record {0} hash does not match its contents")]
    HashMismatch(usize),
    #[error("record {0} prev_hash does not match the previous record")]
    BrokenLink(usize),
    #[error("record {0} has an invalid checker signature")]
    BadCheckerSignature(usize),
}

impl AuditChain {
    pub fn new() -> Self {
        AuditChain {
            records: Vec::new(),
        }
    }

    /// The hash to chain the next record onto: the last entry's `this_hash`,
    /// or `GENESIS` when empty.
    pub fn head(&self) -> Hash {
        self.records
            .last()
            .map(|r| r.this_hash())
            .unwrap_or(GENESIS)
    }

    /// Append an entry. Its `prev_hash` must equal the current head.
    pub fn append(&mut self, entry: AuditEntry) -> Result<(), ChainError> {
        if entry.prev_hash() != self.head() {
            return Err(ChainError::WrongPrevHash);
        }
        self.records.push(entry);
        Ok(())
    }

    pub fn records(&self) -> &[AuditEntry] {
        &self.records
    }
}

/// Verify an entire chain: every entry's hash matches its contents, every link
/// matches the previous entry, and every checker/approver signature verifies.
/// Any byte mutation anywhere fails this (invariant #6). Merge signatures are
/// checked against the diff hash, dispatch approvals against the prompt hash.
/// An abandoned dispatch with zero approvers verifies on the hash chain alone.
pub fn verify_chain(records: &[AuditEntry]) -> Result<(), ChainBreak> {
    let mut expected_prev = GENESIS;
    for (i, entry) in records.iter().enumerate() {
        if entry.recompute_hash() != entry.this_hash() {
            return Err(ChainBreak::HashMismatch(i));
        }
        if entry.prev_hash() != expected_prev {
            return Err(ChainBreak::BrokenLink(i));
        }
        let (gate_id, diff_hash, checkers) = entry.signed_over();
        for checker in checkers {
            if !verify_entry_signature(gate_id, diff_hash, checker) {
                return Err(ChainBreak::BadCheckerSignature(i));
            }
        }
        expected_prev = entry.this_hash();
    }
    Ok(())
}

/// Verify one checker/approver signature against the content it bound to.
fn verify_entry_signature(gate_id: &GateId, diff_hash: Hash, checker: &CheckerEntry) -> bool {
    let content = SignedContent {
        gate_id: gate_id.clone(),
        diff_hash,
        operator: checker.operator,
        verdict: checker.verdict.clone(),
        depth: checker.depth,
        cast_at: checker.cast_at,
    };
    verify(
        checker.operator,
        &canonical_bytes(&content),
        &checker.signature,
    )
}

/// The operators whose verified `go` signatures back this entry — the source
/// of the `Reviewed-by:` trailers (FR-21). Works for either entry kind.
pub fn reviewed_by(entry: &AuditEntry) -> Vec<OperatorId> {
    let (gate_id, diff_hash, checkers) = entry.signed_over();
    checkers
        .iter()
        .filter(|c| c.verdict == Verdict::Go && verify_entry_signature(gate_id, diff_hash, c))
        .map(|c| c.operator)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::record::{GateRecord, Provenance};
    use crate::eligibility::MakerSet;
    use crate::hold::DualHold;
    use crate::ids::{GateId, TaskId};
    use crate::policy::{Authorship, Outcome};
    use crate::sign::{Ed25519Signer, FixedClock, Signer};
    use crate::verdict::{CastVerdict, Remedy, Verdict};
    use crate::{GatePolicy, ReviewDepth};

    fn record(prev: Hash, gate: &str) -> AuditEntry {
        AuditEntry::Merge(merge_record(prev, gate))
    }

    fn merge_record(prev: Hash, gate: &str) -> GateRecord {
        let mut h = DualHold::new(
            GateId(gate.into()),
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
        let prov = Provenance {
            task_id: TaskId("t1".into()),
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
        GateRecord::build(prev, prov, &h).unwrap()
    }

    #[test]
    fn append_and_verify_two_record_chain() {
        let mut chain = AuditChain::new();
        let r1 = record(GENESIS, "g1");
        chain.append(r1).unwrap();
        let r2 = record(chain.head(), "g2");
        chain.append(r2).unwrap();
        assert!(verify_chain(chain.records()).is_ok());
        assert_eq!(chain.records().len(), 2);
    }

    #[test]
    fn append_rejects_wrong_prev_hash() {
        let mut chain = AuditChain::new();
        let bad = record(Hash([7u8; 32]), "g1"); // prev != GENESIS
        assert_eq!(chain.append(bad).unwrap_err(), ChainError::WrongPrevHash);
    }

    #[test]
    fn mutating_a_record_breaks_verification() {
        let mut chain = AuditChain::new();
        chain.append(record(GENESIS, "g1")).unwrap();
        let mut records = chain.records().to_vec();
        // Tamper with recorded provenance without recomputing the hash.
        let AuditEntry::Merge(r) = &mut records[0] else {
            panic!("expected a merge entry")
        };
        r.core.provenance.loc = 999;
        assert_eq!(
            verify_chain(&records).unwrap_err(),
            ChainBreak::HashMismatch(0)
        );
    }

    #[test]
    fn reviewed_by_lists_both_go_signers() {
        let r = record(GENESIS, "g1");
        let signers = reviewed_by(&r);
        assert_eq!(signers.len(), 2);
        assert!(signers.contains(&Ed25519Signer::from_seed([1; 32]).operator_id()));
        assert!(signers.contains(&Ed25519Signer::from_seed([2; 32]).operator_id()));
    }

    #[test]
    fn verify_chain_detects_broken_link() {
        // Two individually-valid records, but the second points at the wrong prev.
        let r1 = record(GENESIS, "g1");
        let r2 = record(Hash([7u8; 32]), "g2"); // valid hash over its own core, wrong prev link
        assert_eq!(
            verify_chain(&[r1, r2]).unwrap_err(),
            ChainBreak::BrokenLink(1)
        );
    }

    #[test]
    fn verify_chain_detects_tampered_checker_signature() {
        // Flip a checker's verdict, then recompute this_hash so the hash check passes.
        // The signature no longer matches the tampered content -> BadCheckerSignature.
        let mut r = merge_record(GENESIS, "g1");
        r.core.checkers[0].verdict = Verdict::NoGo(Remedy::Steer("forged".into()));
        r.this_hash = r.recompute_hash();
        assert_eq!(
            verify_chain(&[AuditEntry::Merge(r)]).unwrap_err(),
            ChainBreak::BadCheckerSignature(0)
        );
    }

    #[test]
    fn verify_chain_accepts_empty() {
        assert!(verify_chain(&[]).is_ok());
    }

    // --- dispatch entries ---------------------------------------------------

    use crate::audit::record::{prompt_hash, DispatchOutcome, DispatchRecord};
    use crate::dispatch_gate_id;

    fn dispatch_hold(prompt: &str) -> DualHold {
        let mut h = DualHold::new(
            dispatch_gate_id(),
            TaskId("dispatch".into()),
            prompt_hash(prompt),
            GatePolicy {
                blind: false,
                ..GatePolicy::default()
            },
            MakerSet::new(),
            Authorship::HandEdited,
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

    fn dispatch_entry(prev: Hash, prompt: &str) -> AuditEntry {
        let h = dispatch_hold(prompt);
        let author = Ed25519Signer::from_seed([1; 32]).operator_id();
        AuditEntry::Dispatch(
            DispatchRecord::build_dispatched(prev, prompt.into(), author, &h).unwrap(),
        )
    }

    #[test]
    fn mixed_dispatch_and_merge_chain_verifies() {
        let mut chain = AuditChain::new();
        chain
            .append(dispatch_entry(GENESIS, "ship the login fix"))
            .unwrap();
        chain.append(record(chain.head(), "g1")).unwrap();
        assert!(verify_chain(chain.records()).is_ok());
        assert_eq!(chain.records().len(), 2);
        // The dispatch entry backs two go-signers via reviewed_by too.
        assert_eq!(reviewed_by(&chain.records()[0]).len(), 2);
    }

    #[test]
    fn tampering_dispatch_prompt_breaks_the_hash() {
        let mut entry = dispatch_entry(GENESIS, "original prompt");
        let AuditEntry::Dispatch(d) = &mut entry else {
            panic!("expected a dispatch entry")
        };
        d.core.prompt = "a different prompt".into(); // hash not recomputed
        assert_eq!(
            verify_chain(&[entry]).unwrap_err(),
            ChainBreak::HashMismatch(0)
        );
    }

    #[test]
    fn tampering_dispatch_prompt_with_rehash_fails_signature() {
        // Recompute this_hash after swapping the prompt: the hash check passes,
        // but the approvers signed the *original* prompt, so signatures fail.
        let mut entry = dispatch_entry(GENESIS, "original prompt");
        let AuditEntry::Dispatch(d) = &mut entry else {
            panic!("expected a dispatch entry")
        };
        d.core.prompt = "a different prompt".into();
        d.this_hash = d.recompute_hash();
        assert_eq!(
            verify_chain(&[entry]).unwrap_err(),
            ChainBreak::BadCheckerSignature(0)
        );
    }

    #[test]
    fn abandoned_dispatch_with_no_approvers_verifies_on_chain_alone() {
        let author = Ed25519Signer::from_seed([1; 32]).operator_id();
        let rec =
            DispatchRecord::build_abandoned(GENESIS, "half-typed prompt".into(), author, vec![]);
        assert_eq!(rec.core.outcome, DispatchOutcome::Abandoned);
        assert!(rec.core.approvers.is_empty());
        assert!(verify_chain(&[AuditEntry::Dispatch(rec)]).is_ok());
    }

    #[test]
    fn chain_with_blocked_record_verifies_and_detects_tamper() {
        let mut chain = AuditChain::new();
        chain.append(record(GENESIS, "g1")).unwrap();
        // blocked record chained after the satisfied one
        let h = {
            let mut h = DualHold::new(
                GateId("g2".into()),
                TaskId("t2".into()),
                Hash([9u8; 32]),
                GatePolicy::default(),
                MakerSet::new(),
                Authorship::Agent,
            );
            for (seed, v) in [
                (1u8, Verdict::Go),
                (2u8, Verdict::NoGo(crate::Remedy::Steer("fix".into()))),
            ] {
                let s = Ed25519Signer::from_seed([seed; 32]);
                let cv = CastVerdict::create(
                    &s,
                    &FixedClock(1000 + seed as i64),
                    h.gate_id(),
                    h.diff_hash(),
                    v,
                    ReviewDepth::FullDiff,
                    None,
                );
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
        chain.append(AuditEntry::Merge(rec)).unwrap();
        assert!(verify_chain(chain.records()).is_ok());
        let mut tampered = chain.records().to_vec();
        let AuditEntry::Merge(r) = &mut tampered[1] else {
            panic!("expected a merge entry")
        };
        r.core.outcome = Outcome::Unanimous; // lie about the outcome
        assert!(verify_chain(&tampered).is_err());
    }
}
