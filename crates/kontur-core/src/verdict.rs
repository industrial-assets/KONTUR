use serde::{Deserialize, Serialize};

use crate::ids::HandEditRef;

/// The corrective payload a `NoGo` must carry. Invariant #4: a `NoGo` cannot
/// exist without a remedy, so a bare veto is not representable.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Remedy {
    /// A corrective prompt sent back to the agent.
    Steer(String),
    /// A reference to a direct human change.
    HandEdit(HandEditRef),
}

/// An operator's decision at a gate.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Verdict {
    Go,
    NoGo(Remedy),
}

impl Verdict {
    pub fn is_go(&self) -> bool {
        matches!(self, Verdict::Go)
    }
}

/// How deeply the checker reviewed, captured for the audit record (PRD §9).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ReviewDepth {
    FullDiff,
    Summary,
    TestsRun,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::HandEditRef;

    #[test]
    fn nogo_always_carries_a_remedy() {
        // A NoGo must be constructed with a Remedy — there is no bare-veto variant.
        let v = Verdict::NoGo(Remedy::Steer("cache the token lookup".into()));
        assert!(!v.is_go());
        match v {
            Verdict::NoGo(Remedy::Steer(s)) => assert_eq!(s, "cache the token lookup"),
            _ => panic!("expected a steer remedy"),
        }

        let v2 = Verdict::NoGo(Remedy::HandEdit(HandEditRef("edit-1".into())));
        assert!(!v2.is_go());
    }

    #[test]
    fn go_is_go() {
        assert!(Verdict::Go.is_go());
    }
}
