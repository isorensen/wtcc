//! TDD RED for issue #44 — the pure `vcs::badge_severity` classifier.
//!
//! Maps a `VcsStatus` to a single `BadgeSeverity` used to color the sidebar PR
//! badge. Ranking (most → least severe): Bad > Pending > Ok > Dirty > None.
//! `status_badge` (the String suffix) is unchanged; this only adds the color
//! role.

use wtcc::vcs::{BadgeSeverity, ChecksState, PrState, PrStatus, VcsStatus, badge_severity};

fn pr(state: PrState, checks: ChecksState) -> PrStatus {
    PrStatus {
        number: 42,
        state,
        checks,
    }
}

#[test]
fn clean_no_pr_is_none() {
    assert_eq!(badge_severity(&VcsStatus::default()), BadgeSeverity::None);
}

#[test]
fn dirty_only_is_dirty() {
    let s = VcsStatus {
        dirty: true,
        pr: None,
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Dirty);
}

#[test]
fn open_passing_pr_is_ok() {
    let s = VcsStatus {
        dirty: false,
        pr: Some(pr(PrState::Open, ChecksState::Passing)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Ok);
}

#[test]
fn open_pending_pr_is_pending() {
    let s = VcsStatus {
        dirty: false,
        pr: Some(pr(PrState::Open, ChecksState::Pending)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Pending);
}

#[test]
fn open_failing_pr_is_bad() {
    let s = VcsStatus {
        dirty: false,
        pr: Some(pr(PrState::Open, ChecksState::Failing)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Bad);
}

#[test]
fn closed_pr_is_bad() {
    let s = VcsStatus {
        dirty: false,
        pr: Some(pr(PrState::Closed, ChecksState::None)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Bad);
}

#[test]
fn bad_outranks_dirty_and_pending() {
    // A failing PR on a dirty tree must read as Bad — the worst state wins.
    let s = VcsStatus {
        dirty: true,
        pr: Some(pr(PrState::Open, ChecksState::Failing)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Bad);
}

#[test]
fn pending_outranks_dirty() {
    // Pending checks on a dirty tree read as Pending, not Dirty.
    let s = VcsStatus {
        dirty: true,
        pr: Some(pr(PrState::Open, ChecksState::Pending)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Pending);
}

#[test]
fn ok_outranks_dirty() {
    // A passing PR on a dirty tree reads as Ok — PR health beats local dirt.
    let s = VcsStatus {
        dirty: true,
        pr: Some(pr(PrState::Open, ChecksState::Passing)),
    };
    assert_eq!(badge_severity(&s), BadgeSeverity::Ok);
}

#[test]
fn severity_ranking_is_total_and_descending() {
    // The enum encodes the ranking directly, so the renderer can pick the most
    // severe role without ad-hoc comparisons: Bad > Pending > Ok > Dirty > None.
    use BadgeSeverity::{Bad, Dirty, None, Ok, Pending};
    assert!(Bad > Pending);
    assert!(Pending > Ok);
    assert!(Ok > Dirty);
    assert!(Dirty > None);
}
