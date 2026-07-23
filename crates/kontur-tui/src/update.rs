//! In-app update check. A calm, fail-silent GitHub Releases probe that tells
//! an operator when a newer kontur exists, plus the pure helpers that decide
//! what the footer says. No telemetry, no code leaves the host — one GET.

use serde::{Deserialize, Serialize};

/// The 24h freshness window for the on-disk check cache, in seconds.
const CACHE_TTL_SECS: u64 = 86_400;

/// Persisted between runs at `~/.kontur/update-check.json`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UpdateCache {
    pub last_checked: u64,
    pub latest_version: String,
}

/// The footer line when a newer release exists, else `None`. Strict semver
/// greater-than; a `v` prefix and unparseable input are handled gracefully.
pub fn upgrade_notice(current: &str, latest: &str) -> Option<String> {
    let cur = semver::Version::parse(current.trim_start_matches('v')).ok()?;
    let lat = semver::Version::parse(latest.trim_start_matches('v')).ok()?;
    (lat > cur).then(|| format!("v{lat} available — brew upgrade kontur"))
}

/// The footer line when the two seats run different releases, else `None`.
/// Advisory only — never gates anything. Empty strings (an old peer that did
/// not send a version) yield no notice.
pub fn peer_version_notice(own: &str, peer: &str) -> Option<String> {
    if own.is_empty() || peer.is_empty() || own == peer {
        return None;
    }
    Some(format!("peer v{peer} · you v{own} — align versions"))
}

/// Whether a cache timestamped at `last_checked_secs` is still fresh at `now_secs`.
pub fn cache_is_fresh(last_checked_secs: u64, now_secs: u64) -> bool {
    now_secs.saturating_sub(last_checked_secs) < CACHE_TTL_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_notice_when_newer() {
        assert_eq!(
            upgrade_notice("0.1.0", "0.2.0"),
            Some("v0.2.0 available — brew upgrade kontur".to_string())
        );
    }

    #[test]
    fn no_upgrade_notice_when_equal_or_older() {
        assert_eq!(upgrade_notice("0.2.0", "0.2.0"), None);
        assert_eq!(upgrade_notice("0.2.0", "0.1.9"), None);
    }

    #[test]
    fn upgrade_notice_tolerates_v_prefix_and_junk() {
        assert_eq!(
            upgrade_notice("0.1.0", "v0.2.0"),
            Some("v0.2.0 available — brew upgrade kontur".to_string())
        );
        assert_eq!(upgrade_notice("0.1.0", "not-a-version"), None);
        assert_eq!(upgrade_notice("garbage", "0.2.0"), None);
    }

    #[test]
    fn peer_notice_only_when_present_and_different() {
        assert_eq!(
            peer_version_notice("0.1.0", "0.1.1"),
            Some("peer v0.1.1 · you v0.1.0 — align versions".to_string())
        );
        assert_eq!(peer_version_notice("0.1.0", "0.1.0"), None);
        assert_eq!(peer_version_notice("0.1.0", ""), None);
        assert_eq!(peer_version_notice("", "0.1.1"), None);
    }

    #[test]
    fn cache_freshness_window_is_24h() {
        assert!(cache_is_fresh(1_000, 1_000 + 86_399));
        assert!(!cache_is_fresh(1_000, 1_000 + 86_400));
    }
}
