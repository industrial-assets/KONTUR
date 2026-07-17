use kontur_core::{
    reviewed_by, verify_chain, AuditChain, Authorship, CastVerdict, DualHold, Ed25519Signer,
    FixedClock, GateId, GatePolicy, GateRecord, Hash, HoldState, Independence, MakerSet, Outcome,
    Provenance, Remedy, ReviewDepth, Signer, TaskId, Verdict, GENESIS,
};

fn provenance(diff: Hash, author: kontur_core::OperatorId) -> Provenance {
    Provenance {
        task_id: TaskId("t1".into()),
        prompt: "refactor session guard to token store".into(),
        prompt_author: author,
        agent_id: "agent-03".into(),
        agent_model: "claude-opus-4-8".into(),
        agent_version: "1.0".into(),
        diff_hash: diff,
        files: vec!["auth/session.ts".into()],
        loc: 59,
        tokens: 6400,
    }
}

fn go(seed: u8, h: &DualHold) -> CastVerdict {
    let signer = Ed25519Signer::from_seed([seed; 32]);
    let clock = FixedClock(1000 + seed as i64);
    CastVerdict::create(
        &signer,
        &clock,
        h.gate_id(),
        h.diff_hash(),
        Verdict::Go,
        ReviewDepth::FullDiff,
        None,
    )
}

fn nogo(seed: u8, h: &DualHold, remedy: Remedy) -> CastVerdict {
    let signer = Ed25519Signer::from_seed([seed; 32]);
    let clock = FixedClock(2000 + seed as i64);
    CastVerdict::create(
        &signer,
        &clock,
        h.gate_id(),
        h.diff_hash(),
        Verdict::NoGo(remedy),
        ReviewDepth::FullDiff,
        None,
    )
}

// UX §7: "Clean task" — dispatch → both go → merge, calm throughout.
#[test]
fn clean_task_produces_a_verified_record() {
    let diff = Hash([9u8; 32]);
    let mut h = DualHold::new(
        GateId("g1".into()),
        TaskId("t1".into()),
        diff,
        GatePolicy::default(),
        MakerSet::new(),
        Authorship::Agent,
    );
    h.cast(0, go(1, &h)).unwrap();
    h.cast(1, go(2, &h)).unwrap();
    assert_eq!(h.state(), HoldState::Satisfied);

    let author = Ed25519Signer::from_seed([1; 32]).operator_id();
    let mut chain = AuditChain::new();
    let rec = GateRecord::build(chain.head(), provenance(diff, author), &h).unwrap();
    chain.append(rec).unwrap();

    assert!(verify_chain(chain.records()).is_ok());
    assert_eq!(rec_outcome(&chain), Outcome::Unanimous);
    assert_eq!(reviewed_by(&chain.records()[0]).len(), 2);
}

fn rec_outcome(chain: &AuditChain) -> Outcome {
    chain.records()[0].core.outcome
}

// UX §7: "Caught in review" — no-go with a steer, then a clean second pass on a
// re-opened (contested) hold → resolved-after-disagreement.
#[test]
fn caught_in_review_records_resolved_after_disagreement() {
    let diff = Hash([9u8; 32]);
    // First pass: navigator casts no-go with a steer.
    let mut first = DualHold::new(
        GateId("g1".into()),
        TaskId("t1".into()),
        diff,
        GatePolicy::default(),
        MakerSet::new(),
        Authorship::Agent,
    );
    first.cast(0, go(1, &first)).unwrap();
    let steer = Remedy::Steer("cache the token lookup".into());
    first.cast(1, nogo(2, &first, steer)).unwrap();
    assert_eq!(first.state(), HoldState::Blocked);
    assert!(first.blocking_remedy().is_some());

    // Agent reworks; second pass on a fresh contested hold over the new diff.
    let diff2 = Hash([10u8; 32]);
    let mut second = DualHold::reopen(
        GateId("g1".into()),
        TaskId("t1".into()),
        diff2,
        GatePolicy::default(),
        MakerSet::new(),
        Authorship::Agent,
    );
    second.cast(0, go(1, &second)).unwrap();
    second.cast(1, go(2, &second)).unwrap();
    assert_eq!(second.outcome(), Some(Outcome::ResolvedAfterDisagreement));
}

// UX §7: "Emergency" — hand-edit applied; pragmatic mode lets the editor
// co-sign; combined diff re-signed by both before merge; authorship flagged.
#[test]
fn emergency_handedit_pragmatic_merges_with_both_authorship() {
    let a = Ed25519Signer::from_seed([1; 32]).operator_id();
    let b = Ed25519Signer::from_seed([2; 32]).operator_id();
    let policy = GatePolicy {
        independence: Independence::Pragmatic,
        ..GatePolicy::default()
    };
    let mut h = DualHold::reopen_handedit(
        GateId("g1".into()),
        TaskId("t1".into()),
        Hash([11u8; 32]),
        policy,
        MakerSet::new(),
        a,
        true,
        &[a, b],
    );
    assert_eq!(h.authorship(), Authorship::Both);
    h.cast(0, go(1, &h)).unwrap(); // editor A co-signs (pragmatic)
    h.cast(1, go(2, &h)).unwrap();
    assert_eq!(h.state(), HoldState::Satisfied);
    assert_eq!(h.authorship(), Authorship::Both);
}

// Determinism: identical inputs (fixed clock + seeded keys) yield byte-identical
// record hashes across independent runs — audit reproducibility.
#[test]
fn records_are_deterministic() {
    fn build_once() -> Hash {
        let diff = Hash([9u8; 32]);
        let mut h = DualHold::new(
            GateId("g1".into()),
            TaskId("t1".into()),
            diff,
            GatePolicy::default(),
            MakerSet::new(),
            Authorship::Agent,
        );
        h.cast(0, go(1, &h)).unwrap();
        h.cast(1, go(2, &h)).unwrap();
        let author = Ed25519Signer::from_seed([1; 32]).operator_id();
        GateRecord::build(GENESIS, provenance(diff, author), &h)
            .unwrap()
            .this_hash
    }
    assert_eq!(build_once(), build_once());
}
