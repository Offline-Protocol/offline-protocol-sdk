//! How old a signed control frame is allowed to be.
//!
//! [`control_signing_payload_v2`] puts the frame's timestamp inside the
//! signature. This module is what reads it back: the windows the two ends
//! allow, and the one comparison that turns a stamp and a clock into a
//! verdict.
//!
//! # Why the windows live beside each other
//!
//! A phone and a leaf node allow *different* ages, and that difference is a
//! shared fact rather than each end's private policy. A frame the phone is
//! still willing to retransmit is a frame the leaf may be asked to verify, so
//! a leaf window shorter than the phone's retransmission horizon is a pair
//! that refuses its own traffic. Keeping both numbers in one file is what
//! makes that relationship checkable; splitting them is how one end is later
//! retuned alone and the refusals show up on a device.
//!
//! [`control_signing_payload_v2`]: crate::canonical::control_signing_payload_v2

/// How far in the past a control frame's timestamp may sit before a phone
/// refuses it.
///
/// Thirty days, which is not a comfort margin but the smallest number that
/// clears every path on which this protocol delivers a *legitimately* old
/// frame:
///
/// - **The outbox retransmits frozen signed bytes.** An entry is retried for
///   `outbox_max_lifetime_ms` (7 days by default) and each probe refreshes its
///   last-send stamp, so terminal failure moves out to the absolute cap of
///   four lifetimes, about 28 days. A connection request riding that ladder is
///   signed once, at the start.
/// - **A published key package is not delivered, it is left to be found.** It
///   sits on a relay for as long as the package is valid: 30 days for one this
///   install minted, and up to [`crate::MAX_ACCEPTED_KEY_PACKAGE_LIFETIME`] for one
///   minted elsewhere. Such a record is signed under the older payload on
///   purpose, so this window is not what admits it; the numbers are here
///   because they are what a window would have to clear if that ever changed.
///
/// A window shorter than either of those refuses frames that are late by
/// design. This is therefore a bound on the replay of every other control
/// frame rather than a closure of it: what closes the destructive case is the
/// spent-frame record the engine keeps for session resets, which only has to
/// reach back as far as this window for the two together to be complete.
///
/// An install that raises `outbox_max_lifetime_ms` past a quarter of this
/// number pushes its own retransmissions outside the window; the delivery
/// documentation carries that caveat.
pub const CTRL_FRESHNESS_PAST_MS: u64 = 30 * 24 * 3600 * 1000;

/// How far into the future a control frame's timestamp may sit before a
/// verifier refuses it.
///
/// Two days. The field is the *sender's* clock reading, so the allowance is
/// for honest disagreement between two devices, not for delivery: nothing
/// legitimate arrives before it was sent. It is generous because the cost of
/// being wrong is asymmetric. A window too small refuses a real peer whose
/// clock is fast, and the refusal looks exactly like an attack; a window too
/// large only lets an attacker who is *already holding a valid signature*
/// choose when within two days to spend it, which the spent-frame record
/// denies separately.
///
/// It is deliberately not zero-tolerance: a receiver that demands `stamp <=
/// now` refuses its own peers over a second of NTP disagreement.
pub const CTRL_FRESHNESS_FUTURE_MS: u64 = 2 * 24 * 3600 * 1000;

/// How far in the past a control frame's timestamp may sit before a **leaf
/// node** refuses it.
///
/// Two days, against the phone's thirty, because a leaf is on the other end of
/// a different kind of link. The paths that make a phone accept a month-old
/// frame do not reach a device: a leaf is paired with one phone over a direct
/// radio link, it is not addressed through a relay, and nothing parks a frame
/// bound for it in an outbox ladder measured in weeks. What a leaf does see
/// late is a retransmission over a link that dropped, which is minutes.
///
/// The shorter window is what makes the device's bounded memory sufficient
/// rather than merely helpful. A leaf remembers only the last few reset frames
/// it acted on, because it has a few hundred kilobytes of flash and no room to
/// grow that list with traffic; a two-day window means the frames that list
/// has to cover are only the ones a peer could have minted in two days, and a
/// phone's rekey cadence is far slower than that. Widening this window without
/// widening that list reopens the gap between them.
pub const LEAF_CTRL_FRESHNESS_PAST_MS: u64 = 2 * 24 * 3600 * 1000;

/// What a verifier concluded about a control frame's age.
///
/// Deliberately three-valued rather than a boolean. The two refusals are the
/// same decision and different diagnoses: one is a frame that waited too long,
/// which is what a replay looks like, and the other is a frame from a clock
/// that disagrees with ours, which is what a misconfigured device looks like.
/// A caller that renders them identically leaves an integrator unable to tell
/// a fleet with a broken time source from a fleet under attack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Inside both windows.
    Fresh,
    /// Older than the past window allows.
    Stale {
        /// How far past the window the frame sits, in milliseconds.
        age_ms: u64,
    },
    /// Stamped further ahead of the verifier's clock than the future window
    /// allows.
    FromTheFuture {
        /// How far ahead of the verifier's clock the stamp sits, in
        /// milliseconds.
        skew_ms: u64,
    },
}

impl Freshness {
    /// Whether the frame may be acted on.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        matches!(self, Freshness::Fresh)
    }
}

/// Judges a control frame's timestamp against a verifier's clock.
///
/// `signed_ms` is the value that was inside the signature, so it is chosen by
/// whoever produced the frame and is bounded by nothing: every arithmetic step
/// here has to survive `i64::MIN` and `i64::MAX` being handed to it. That is
/// why the difference saturates rather than wrapping, and why the magnitude is
/// taken with `unsigned_abs`, which is defined at `i64::MIN` where negation is
/// not.
///
/// Both windows are parameters rather than reads of the constants above,
/// because the two ends allow different ages and a function that hard-coded
/// one of them would need a second copy for the other. That is the failure
/// this crate exists to prevent.
#[must_use]
pub fn control_frame_freshness(
    signed_ms: i64,
    now_ms: i64,
    past_window_ms: u64,
    future_window_ms: u64,
) -> Freshness {
    let delta = now_ms.saturating_sub(signed_ms);
    if delta >= 0 {
        let age_ms = delta.unsigned_abs();
        if age_ms > past_window_ms {
            Freshness::Stale { age_ms }
        } else {
            Freshness::Fresh
        }
    } else {
        let skew_ms = delta.unsigned_abs();
        if skew_ms > future_window_ms {
            Freshness::FromTheFuture { skew_ms }
        } else {
            Freshness::Fresh
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    #[test]
    fn a_frame_stamped_now_is_fresh() {
        assert_eq!(
            control_frame_freshness(NOW, NOW, CTRL_FRESHNESS_PAST_MS, CTRL_FRESHNESS_FUTURE_MS),
            Freshness::Fresh
        );
    }

    /// The boundary is inclusive: a frame exactly at the window's edge is
    /// still accepted, so a window of exactly the outbox's absolute cap does
    /// not refuse the last retransmission it is supposed to cover.
    #[test]
    fn the_past_window_edge_is_inclusive() {
        let at_edge = NOW - CTRL_FRESHNESS_PAST_MS as i64;
        assert_eq!(
            control_frame_freshness(
                at_edge,
                NOW,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::Fresh
        );
        assert_eq!(
            control_frame_freshness(
                at_edge - 1,
                NOW,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::Stale {
                age_ms: CTRL_FRESHNESS_PAST_MS + 1
            }
        );
    }

    #[test]
    fn the_future_window_edge_is_inclusive() {
        let at_edge = NOW + CTRL_FRESHNESS_FUTURE_MS as i64;
        assert_eq!(
            control_frame_freshness(
                at_edge,
                NOW,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::Fresh
        );
        assert_eq!(
            control_frame_freshness(
                at_edge + 1,
                NOW,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::FromTheFuture {
                skew_ms: CTRL_FRESHNESS_FUTURE_MS + 1
            }
        );
    }

    /// The whole point: a frame older than the window is refused however
    /// valid its signature is.
    #[test]
    fn a_captured_frame_goes_stale() {
        let a_year_ago = NOW - 365 * 24 * 3600 * 1000;
        assert!(!control_frame_freshness(
            a_year_ago,
            NOW,
            CTRL_FRESHNESS_PAST_MS,
            CTRL_FRESHNESS_FUTURE_MS
        )
        .is_fresh());
    }

    /// A leaf refuses what a phone would still accept, which is the asymmetry
    /// the two windows exist to express.
    #[test]
    fn a_leaf_window_refuses_what_a_phone_admits() {
        let ten_days_ago = NOW - 10 * 24 * 3600 * 1000;
        assert!(control_frame_freshness(
            ten_days_ago,
            NOW,
            CTRL_FRESHNESS_PAST_MS,
            CTRL_FRESHNESS_FUTURE_MS
        )
        .is_fresh());
        assert!(!control_frame_freshness(
            ten_days_ago,
            NOW,
            LEAF_CTRL_FRESHNESS_PAST_MS,
            CTRL_FRESHNESS_FUTURE_MS
        )
        .is_fresh());
    }

    /// The stamp is attacker-chosen, so the extremes of the type must produce
    /// a refusal rather than a panic or a wrapped comparison that reads as
    /// fresh. A debug build would panic on overflow, which on a device is a
    /// remote reset.
    #[test]
    fn the_extremes_of_the_type_refuse_rather_than_overflow() {
        assert!(matches!(
            control_frame_freshness(
                i64::MIN,
                NOW,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::Stale { .. }
        ));
        assert!(matches!(
            control_frame_freshness(
                i64::MAX,
                NOW,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::FromTheFuture { .. }
        ));
        // And with the clock itself at an extreme, which a device with no
        // time source can report.
        assert!(matches!(
            control_frame_freshness(
                i64::MAX,
                i64::MIN,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::FromTheFuture { .. }
        ));
        assert!(matches!(
            control_frame_freshness(
                i64::MIN,
                i64::MAX,
                CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS
            ),
            Freshness::Stale { .. }
        ));
    }

    /// A zero-length window admits only the present instant, which is what
    /// makes the comparison a `>` rather than a `>=`.
    #[test]
    fn a_zero_window_admits_only_the_instant_itself() {
        assert_eq!(control_frame_freshness(NOW, NOW, 0, 0), Freshness::Fresh);
        assert!(!control_frame_freshness(NOW - 1, NOW, 0, 0).is_fresh());
        assert!(!control_frame_freshness(NOW + 1, NOW, 0, 0).is_fresh());
    }

    /// The windows are the numbers the documentation states, and both ends
    /// build against them.
    #[test]
    fn the_windows_have_their_published_values() {
        assert_eq!(CTRL_FRESHNESS_PAST_MS, 2_592_000_000);
        assert_eq!(CTRL_FRESHNESS_FUTURE_MS, 172_800_000);
        assert_eq!(LEAF_CTRL_FRESHNESS_PAST_MS, 172_800_000);
    }

    /// The phone's window has to clear the longest legitimate delivery delay
    /// the engine can produce: four times the default outbox lifetime.
    #[test]
    fn the_phone_window_covers_the_outbox_absolute_cap() {
        const DEFAULT_OUTBOX_LIFETIME_MS: u64 = 7 * 24 * 3600 * 1000;
        const ABSOLUTE_CAP_FACTOR: u64 = 4;
        assert!(CTRL_FRESHNESS_PAST_MS >= DEFAULT_OUTBOX_LIFETIME_MS * ABSOLUTE_CAP_FACTOR);
    }
}
