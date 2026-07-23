//! End-to-end coverage for the `kontur audit` CLI, including the back-compat
//! shim: audit files written before the dispatch-gate record existed are a bare
//! `Vec<GateRecord>` (untagged), and must still verify.

use std::process::Command;

use kontur_core::{
    dispatch_gate_id, prompt_hash, AuditEntry, Authorship, CastVerdict, DispatchRecord, DualHold,
    Ed25519Signer, FixedClock, GateId, GatePolicy, GateRecord, Hash, MakerSet, Provenance,
    ReviewDepth, Signer, TaskId, Verdict, GENESIS,
};

fn satisfied_merge(prev: Hash, gate: &str, diff: Hash) -> GateRecord {
    let mut h = DualHold::new(
        GateId(gate.into()),
        TaskId("t1".into()),
        diff,
        GatePolicy::default(),
        MakerSet::new(),
        Authorship::Agent,
    );
    for seed in [1u8, 2u8] {
        let signer = Ed25519Signer::from_seed([seed; 32]);
        let cv = CastVerdict::create(
            &signer,
            &FixedClock(1000 + seed as i64),
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
        diff_hash: diff,
        files: vec!["f".into()],
        loc: 1,
        tokens: 1,
    };
    GateRecord::build(prev, prov, &h).unwrap()
}

fn dispatched(prev: Hash, prompt: &str) -> DispatchRecord {
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
        let cv = CastVerdict::create(
            &signer,
            &FixedClock(1000 + seed as i64),
            h.gate_id(),
            h.diff_hash(),
            Verdict::Go,
            ReviewDepth::FullDiff,
            None,
        );
        let ev = h.version();
        h.cast(ev, cv).unwrap();
    }
    let author = Ed25519Signer::from_seed([1; 32]).operator_id();
    DispatchRecord::build_dispatched(prev, prompt.into(), author, &h).unwrap()
}

fn run_audit(path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kontur"))
        .arg("audit")
        .arg(path)
        .output()
        .expect("run kontur audit")
}

#[test]
fn audits_new_mixed_chain() {
    let dir = std::env::temp_dir().join(format!("kontur-audit-new-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audit.json");

    // Dispatch entry first, then a merge entry chained onto it.
    let disp = dispatched(GENESIS, "ship the fix");
    let merge = satisfied_merge(disp.this_hash, "gate-001", Hash([9u8; 32]));
    let chain = vec![AuditEntry::Dispatch(disp), AuditEntry::Merge(merge)];
    std::fs::write(&path, serde_json::to_vec(&chain).unwrap()).unwrap();

    let out = run_audit(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "audit should pass; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("audit chain OK"), "got: {stdout}");
    assert!(
        stdout.contains("1 dispatch"),
        "dispatch count surfaced; got: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audits_legacy_untagged_gaterecord_file() {
    let dir = std::env::temp_dir().join(format!("kontur-audit-legacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audit-legacy.json");

    // The pre-dispatch on-disk shape: a bare Vec<GateRecord> with no "kind" tag.
    let r1 = satisfied_merge(GENESIS, "gate-001", Hash([9u8; 32]));
    let r2 = satisfied_merge(r1.this_hash, "gate-002", Hash([7u8; 32]));
    let legacy = vec![r1, r2];
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    let out = run_audit(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "legacy audit file must still verify; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("audit chain OK"), "got: {stdout}");
    assert!(stdout.contains("2 gates"), "got: {stdout}");

    let _ = std::fs::remove_dir_all(&dir);
}
