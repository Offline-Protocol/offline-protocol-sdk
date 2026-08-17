//! Username discovery: publishing this install's claim, and resolving a name
//! to the set of devices claiming it.
//!
//! The publication half mirrors [`super::nostr_publication`] closely enough
//! that the differences are the interesting part:
//!
//! - **One claim, not a set of slots.** A key package is single-use, so several
//!   must stand at once. A claim is a statement, so exactly one stands and
//!   republishing *replaces* it.
//! - **A deterministic `d` tag.** That is what makes replacement work, and it
//!   is what makes retraction possible at all.
//! - **Retraction exists.** A key-package slot is abandoned by letting it
//!   expire; a claim must be actively withdrawn, because it names a human and
//!   points at an address that stays live.
//!
//! The resolution half has one rule that outranks its mechanics: **a username
//! resolves to a set, and the set is the only thing ever emitted.** See
//! [`PendingResolution`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use base64::Engine as _;
use offline_protocol_core::Username;
use offline_protocol_mls::discovery::{
    parse_discovery_body, verify_discovery_record, DiscoveryBody, DiscoveryRecordV1,
    DiscoveryTombstoneV1,
};
use offline_protocol_transport::nostr::ResolveRequest;
use offline_protocol_transport::nostr_crypto::discovery_tag_for_username;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::{storage_keys, OfflineProtocol};
use crate::events::{Event, UsernameClaim};
use crate::{Error, Result};

/// How often the discovery claim is re-examined, matching the slot refresh.
const DISCOVERY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Ceiling on the republish backoff, and the quiet period after which a
/// failure streak resets. Matches the key-package publication ladder.
const DISCOVERY_MAX_BACKOFF: Duration = Duration::from_secs(1800);

/// How long a resolution may accumulate before it is flushed regardless.
///
/// End-of-stored-events is the natural completion signal, and it is the one a
/// relay is free not to send. Without this sweep a relay that goes quiet after
/// answering would leave the resolution accumulating forever and the app
/// waiting on an event that never comes — a hang rather than an empty result.
/// Sized well above a relay round-trip and well below any human's patience.
const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum concurrent username resolutions.
///
/// Bounded like every other query-keyed map: a caller can start resolutions
/// faster than relays answer them, and an app looping over a contact list
/// would otherwise grow this without limit. At capacity the *oldest* is
/// flushed with whatever it has rather than dropped silently, so its caller
/// still receives an answer.
///
/// Deliberately *not* the transport's queue ceiling, and named differently to
/// keep the two from being read as one number: that one bounds lookups waiting
/// to be minted into queries, this one bounds accumulators waiting for answers.
/// A lookup passes through both, so the smaller of the two is what a caller
/// actually gets, and equalizing them would only hide which limit was reached.
const MAX_CONCURRENT_RESOLUTIONS: usize = 32;

/// Maximum names for which a tombstone may be owed at once.
///
/// One rename owes one retraction, and each is retired as soon as a relay
/// acknowledges it, so reaching this means retractions have been failing across
/// several renames. The oldest is dropped, because an unbounded list persisted
/// across restarts is worse than an old claim this install can no longer
/// withdraw: hop 2 still arbitrates the stale claim, and nothing arbitrates a
/// record that grows forever.
const MAX_PENDING_RETRACTIONS: usize = 8;

/// Maximum claims accumulated for one username.
///
/// The tag is public, so a squatter can publish many records at it. This caps
/// what one resolution can cost in memory and what an app is asked to render.
/// Reaching it means crowding, which the design accepts: the honest claimants
/// may be among the ones displaced, which is why the invite path exists and
/// why a user confirms out of band.
const MAX_CLAIMS_PER_RESOLUTION: usize = 32;

/// What this install claims, and what it still owes a tombstone for.
///
/// # Why the owed retractions are persisted beside the claim
///
/// A retraction is the one operation here that cannot be reconstructed from
/// anything else. The claim can: it is `config.profile`, so a lost record costs
/// one redundant publish. The *old* name is gone the moment the profile
/// changes, so a record that forgets it before the tombstone lands leaves a
/// human-readable name standing in a public directory, pointing at an address
/// this install is still publishing key packages for. Nothing expires it,
/// because the second hop it dead-ends at is exactly the hop that still works.
///
/// So a name moves from [`Self::claim`] to [`Self::retracting`] and stays there
/// until a relay acknowledges the tombstone. That survives a failed send, a
/// process restart, and the case that has no failure to report at all: no Nostr
/// transport installed, where queueing a retraction is a silent no-op.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct DiscoveryClaimState {
    /// The name currently claimed, if any.
    #[serde(default)]
    pub(crate) claim: Option<Username>,
    /// Names whose tombstone has not yet reached a relay, oldest first.
    #[serde(default)]
    pub(crate) retracting: Vec<Username>,
}

/// A username resolution in flight.
///
/// # Why claims accumulate here instead of being emitted as they arrive
///
/// Emitting one event per claim would make the *first* claim the easiest thing
/// for an app to consume, and the first claim is meaningless — it is whichever
/// relay answered fastest. An app written the obvious way against a stream of
/// per-claim events silently becomes an app that picks a winner, which is
/// precisely the failure this layer must not enable.
///
/// So the set accumulates here and leaves as one event. The shape makes the
/// correct behaviour the path of least resistance rather than a documented
/// obligation, which is the only kind of API rule that survives contact with a
/// deadline.
pub(crate) struct PendingResolution {
    /// The username being resolved.
    username: Username,
    /// Verified claims, keyed by the publishing Nostr author key.
    ///
    /// Keyed by author rather than by address because that is what a device
    /// *is* here: one install publishes under one Nostr key. Two records from
    /// the same author are the same device republishing, and the newer one
    /// wins. Two records from different authors naming the same address would
    /// be unusual (it needs the same identity key on two installs) and are
    /// kept separate, since collapsing them would hide a real anomaly.
    claims: HashMap<String, DiscoveryRecordV1>,
    /// Authors that have retracted, so a retraction survives whatever order the
    /// relays answer in.
    ///
    /// A tombstone and the record it replaces both live on the relays for a
    /// while: the tombstone occupies the addressable slot, and a relay that
    /// missed the replacement still serves the old record. Removing the claim
    /// on arrival is therefore not enough — a stale copy landing afterwards
    /// would verify (it is genuinely signed) and stand the retracted claim back
    /// up, which is precisely the outcome retraction exists to prevent.
    ///
    /// A suppressed record is **not** counted in [`Self::rejected`]. It is not
    /// junk, and counting it would make the same two events report differently
    /// depending on which arrived first, reintroducing the order-dependence in
    /// the counter after removing it from the set.
    ///
    /// Bounded by the transport's per-query delivery ceiling, like `claims`.
    tombstoned: HashSet<String>,
    /// Records seen and refused. Reported so an app can tell "nobody claims
    /// this name" apart from "everything claiming it was junk".
    rejected: u32,
    /// Verified claims dropped at [`MAX_CLAIMS_PER_RESOLUTION`].
    ///
    /// Counted separately from [`Self::rejected`] because it is the opposite
    /// statement: these records passed every check and are missing anyway.
    /// Folding them into the refusal count would report a squatted name as a
    /// junk-filled one, and reporting neither would render it as a clean set.
    truncated: u32,
    /// When the resolution began, driving the timeout sweep.
    started: Instant,
}

impl OfflineProtocol {
    /// Publishes, republishes or retracts this install's username claim.
    ///
    /// Runs on the process tick beside the key-package slot refresh. Four
    /// states, and the transitions between them are the whole of this
    /// function:
    ///
    /// 1. discovery is off and nothing stands: do nothing;
    /// 2. discovery is off and a claim stands: retract it;
    /// 3. discovery is on and the standing claim names a different username
    ///    than the current profile: retract the old one, then publish the new;
    /// 4. discovery is on and the claim is current: republish once per process;
    /// 5. a tombstone is owed, from this run or a previous one: queue it again,
    ///    until a relay acknowledges it.
    ///
    /// State 3 is the one that is easy to omit and expensive to omit. A user
    /// who renames leaves their old name standing in a public directory,
    /// pointing at an address that is still live, with nothing to ever remove
    /// it — and the *new* owner of that name looks like a squatter next to it.
    ///
    /// State 5 is why states 2 and 3 record the debt rather than discharging
    /// it. Queueing a tombstone is not the same as landing one: the send can
    /// fail, the process can exit first, and with no Nostr transport installed
    /// the queue call is a silent no-op. Each of those leaves the same stale
    /// claim as omitting state 3 entirely, and the name is unrecoverable by
    /// then because the profile has already moved on.
    pub(crate) fn refresh_nostr_discovery_claim(&mut self) {
        if !self.nostr_discovery_refresh_due() {
            return;
        }

        let now = Instant::now();

        // Drained after the throttle check, like the slot reports: draining is
        // destructive and a throttled tick that dropped them would lose them.
        //
        // Confirmations before failures, so a tag reported both ways inside one
        // interval settles as landed. The other order would leave a retraction
        // that reached a relay owed forever, re-queued on every tick.
        let confirmed = self
            .transport_manager
            .take_confirmed_nostr_discovery_publications();
        self.retire_confirmed_retractions(&confirmed);

        for tag in self
            .transport_manager
            .take_failed_nostr_discovery_publications()
        {
            // Scoped to the standing claim's own tag. A failed *retraction* of
            // a name this install no longer claims says nothing about whether
            // the current claim is on the relays, and clearing the flag for it
            // republishes a healthy claim on every such failure while the
            // backoff ladder, which is keyed by the failing tag, does not
            // apply.
            if self.discovery_tag_is_claimed(&tag) {
                self.nostr_discovery_published = false;
            }
            self.note_discovery_failure(&tag, now);
        }

        let enabled = self.transport_manager.nostr_discovery_active();
        let desired = if enabled {
            match self.config.profile.parse::<Username>() {
                Ok(username) => Some(username),
                Err(e) => {
                    // Not a warning event: an app whose profile is not a
                    // claimable username has simply not opted into a feature
                    // that needs one, and saying so once per minute would be
                    // noise. The claim is not published, which is the correct
                    // and safe outcome.
                    debug!(
                        error = %e,
                        "Profile is not a claimable username; discovery claim not published"
                    );
                    None
                }
            }
        } else {
            None
        };

        // Retract whatever stands that should not. This only *records* the
        // debt; queueing it is the next step, and is retried from the record
        // until a relay acknowledges it.
        if let Some(standing) = self.nostr_discovery_claim.clone() {
            if desired.as_ref() != Some(&standing) {
                self.owe_retraction(standing);
            }
        }

        self.queue_owed_retractions(desired.as_ref(), now);
        self.prune_discovery_backoff(desired.as_ref());

        let Some(username) = desired else {
            return;
        };

        if self.nostr_discovery_claim.as_ref() == Some(&username) && self.nostr_discovery_published
        {
            return;
        }

        if self.discovery_backoff_active(&username, now) {
            return;
        }

        // Identity readiness is a *timing* state, not a fault: this tick runs
        // from process start, and MLS init plus the Nostr signing key land
        // some ticks later. Warning about it would emit once a minute during
        // every normal startup and train a reader to ignore the one message
        // that means something. Checked before the attempt rather than
        // classified after it, so the two cannot drift.
        if self.mls_manager.is_none()
            || self.local_address().is_none()
            || self.transport_manager.nostr_public_key().is_none()
        {
            debug!("Identity not ready yet; discovery claim deferred to a later tick");
            return;
        }

        if let Err(e) = self.publish_discovery_claim(username) {
            warn!(error = %e, "Failed to publish the Nostr username discovery claim");
        }
    }

    /// Builds, signs and queues the claim record.
    fn publish_discovery_claim(&mut self, username: Username) -> Result<()> {
        let address = self
            .local_address()
            .ok_or_else(|| Error::Other("No local address yet".to_string()))?
            .parse()
            .map_err(|e| Error::Other(format!("Local address is not canonical: {}", e)))?;

        let nostr_author = self
            .transport_manager
            .nostr_public_key()
            .ok_or_else(|| Error::Other("Nostr transport has no signing key yet".to_string()))?;
        let author_bytes = hex::decode(&nostr_author)
            .map_err(|e| Error::Other(format!("Nostr public key is not hex: {}", e)))?;

        let mls = self.mls_manager.as_ref().ok_or(Error::MlsNotInitialized)?;
        let manager = mls
            .read()
            .map_err(|_| Error::Other("MLS lock poisoned".to_string()))?;
        let public_key = manager
            .get_identity_public_key()
            .map_err(|e| Error::Other(format!("Failed to get identity public key: {}", e)))?;

        let record = DiscoveryRecordV1::unsigned(
            username.clone(),
            address,
            public_key,
            author_bytes,
            chrono::Utc::now().timestamp_millis(),
        )
        .sign_with(|payload| manager.sign_data(payload))
        .map_err(|e| Error::Other(format!("Failed to sign the discovery record: {}", e)))?;
        drop(manager);

        let payload =
            serde_json::to_vec(&record).map_err(|e| Error::Serialization(e.to_string()))?;

        self.transport_manager
            .publish_nostr_discovery_record(username.clone(), payload);

        self.nostr_discovery_published = true;

        // Claiming a name cancels any tombstone still owed for it. The two are
        // contradictory statements about one addressable slot and the claim is
        // the newer one, which is also why the transport collapses a queued
        // tombstone for this name into this publication rather than sending
        // both.
        let claim_changed = self.nostr_discovery_claim.as_ref() != Some(&username);
        let debt_cleared = self.remove_owed_retraction(&username);
        if claim_changed {
            self.nostr_discovery_claim = Some(username);
        }
        if claim_changed || debt_cleared {
            self.persist_nostr_discovery_claim();
        }
        Ok(())
    }

    /// Records that a name is owed a tombstone, and stops claiming it.
    ///
    /// Recording is separate from queueing because the two can fail
    /// independently, and only one of them is recoverable from anything else.
    /// The debt is written down first, so every later attempt has a name to
    /// retract: without it the sole record of the old name is gone the moment
    /// the profile changes.
    fn owe_retraction(&mut self, username: Username) {
        if !self.nostr_discovery_retracting.contains(&username) {
            if self.nostr_discovery_retracting.len() >= MAX_PENDING_RETRACTIONS {
                self.nostr_discovery_retracting.remove(0);
                // The name itself is not logged. It is the most personal
                // identifier this module handles, and a device log is not the
                // place to write one down; the count is what a reader needs.
                warn!(
                    cap = MAX_PENDING_RETRACTIONS,
                    "Too many un-landed username retractions; giving up on the \
                     oldest. Its claim may stand until its key packages expire"
                );
            }
            self.nostr_discovery_retracting.push(username);
        }
        self.nostr_discovery_claim = None;
        self.nostr_discovery_published = false;
        self.persist_nostr_discovery_claim();
    }

    /// Drops a name from the owed set, reporting whether it was there.
    fn remove_owed_retraction(&mut self, username: &Username) -> bool {
        let before = self.nostr_discovery_retracting.len();
        self.nostr_discovery_retracting
            .retain(|owed| owed != username);
        self.nostr_discovery_retracting.len() != before
    }

    /// Retires the retractions whose tombstones a relay acknowledged.
    fn retire_confirmed_retractions(&mut self, tags: &[String]) {
        if tags.is_empty() || self.nostr_discovery_retracting.is_empty() {
            return;
        }
        let before = self.nostr_discovery_retracting.len();
        self.nostr_discovery_retracting.retain(|username| {
            // A name whose tag will not derive can never be confirmed either,
            // so keeping it would owe a tombstone forever. It cannot arise from
            // a parsed `Username`, and dropping it is the terminating choice.
            match discovery_tag_for_username(username) {
                Ok(tag) => !tags.contains(&tag),
                Err(_) => false,
            }
        });
        if self.nostr_discovery_retracting.len() != before {
            debug!("A username retraction reached a relay; no longer owed");
            self.persist_nostr_discovery_claim();
        }
    }

    /// Queues a tombstone, and a best-effort deletion, for every name still
    /// owed one.
    ///
    /// Re-queued on every refresh until a relay acknowledges it, which is what
    /// carries a retraction across a failed send, a restart, and a run with no
    /// Nostr transport installed. Idempotent at the relay: a tombstone
    /// republished into its own addressable slot replaces itself.
    fn queue_owed_retractions(&mut self, desired: Option<&Username>, now: Instant) {
        if self.nostr_discovery_retracting.is_empty() {
            return;
        }
        // Nothing here can carry a tombstone, and `retract_nostr_discovery_record`
        // would accept it silently. Keeping the debt is the entire point: it is
        // queued once a transport exists.
        if !self.transport_manager.has_nostr_transport() {
            debug!("No Nostr transport; username retractions stay owed");
            return;
        }

        let owed: Vec<Username> = self
            .nostr_discovery_retracting
            .iter()
            .filter(|username| Some(*username) != desired)
            .filter(|username| !self.discovery_backoff_active(username, now))
            .cloned()
            .collect();

        for username in owed {
            match serde_json::to_vec(&DiscoveryTombstoneV1::new()) {
                Ok(payload) => {
                    self.transport_manager
                        .retract_nostr_discovery_record(username, payload);
                }
                Err(e) => warn!(error = %e, "Failed to build a discovery tombstone"),
            }
        }
    }

    /// Whether `tag` is the tag of the name this install currently claims.
    fn discovery_tag_is_claimed(&self, tag: &str) -> bool {
        self.nostr_discovery_claim
            .as_ref()
            .and_then(|username| discovery_tag_for_username(username).ok())
            .is_some_and(|claimed| claimed == tag)
    }

    /// Drops backoff entries for tags nothing will consult again.
    ///
    /// The ladder is keyed by tag, and a tag becomes unreachable as soon as its
    /// name is neither claimed nor owed a retraction: a rename leaves one
    /// behind on every failure. Unlike the key-package ladder, whose keys are a
    /// fixed slot set, this one would otherwise grow for the life of the
    /// process.
    fn prune_discovery_backoff(&mut self, desired: Option<&Username>) {
        if self.nostr_discovery_backoff.is_empty() {
            return;
        }
        let mut live: HashSet<String> = self
            .nostr_discovery_retracting
            .iter()
            .filter_map(|username| discovery_tag_for_username(username).ok())
            .collect();
        for username in desired.into_iter().chain(self.nostr_discovery_claim.iter()) {
            if let Ok(tag) = discovery_tag_for_username(username) {
                live.insert(tag);
            }
        }
        self.nostr_discovery_backoff
            .retain(|tag, _| live.contains(tag));
    }

    /// Whether enough time has passed to re-examine the claim.
    fn nostr_discovery_refresh_due(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_nostr_discovery_refresh {
            if now.duration_since(last) < DISCOVERY_REFRESH_INTERVAL {
                return false;
            }
        }
        self.last_nostr_discovery_refresh = Some(now);
        true
    }

    /// Records a failed publication and pushes the next attempt out.
    ///
    /// Identical ladder to the key-package slots: the first failure retries on
    /// the next refresh, and only a claim that keeps failing climbs.
    fn note_discovery_failure(&mut self, tag: &str, now: Instant) {
        let entry = self
            .nostr_discovery_backoff
            .entry(tag.to_string())
            .or_insert(super::nostr_publication::PublicationBackoff {
                failures: 0,
                last_failure: now,
                retry_at: now,
            });

        if now.duration_since(entry.last_failure) > DISCOVERY_MAX_BACKOFF {
            entry.failures = 1;
        } else {
            entry.failures = entry.failures.saturating_add(1);
        }
        entry.last_failure = now;

        let delay = if entry.failures <= 1 {
            Duration::ZERO
        } else {
            let shift = (entry.failures - 2).min(16);
            DISCOVERY_REFRESH_INTERVAL
                .saturating_mul(1u32 << shift)
                .min(DISCOVERY_MAX_BACKOFF)
        };
        entry.retry_at = now + delay;
    }

    /// Whether the claim is waiting out a publication backoff.
    fn discovery_backoff_active(&self, username: &Username, now: Instant) -> bool {
        let Ok(tag) =
            offline_protocol_transport::nostr_crypto::discovery_tag_for_username(username)
        else {
            return false;
        };
        self.nostr_discovery_backoff
            .get(&tag)
            .is_some_and(|backoff| now < backoff.retry_at)
    }

    /// Starts a username resolution.
    ///
    /// # The return value means exactly one thing: an answer is coming
    ///
    /// `Ok(true)` started a lookup and `Ok(false)` joined one already in
    /// flight, and **both** are followed by exactly one
    /// [`Event::UsernameResolved`] for the name. Every case where no event will
    /// ever arrive is an error instead.
    ///
    /// That split is the whole point. A caller awaits the event, so folding
    /// "discovery is off" into `false` alongside "already in flight" leaves an
    /// app unable to tell waiting from hanging — and the failure mode is a
    /// spinner that never stops, on the path a user reaches by typing a name
    /// and pressing search.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`] if the string is not a claimable username.
    /// - [`Error::InvalidConfiguration`] if username discovery is off. It also
    ///   requires cold contact, so this covers a claim that could only
    ///   dead-end one hop later.
    /// - [`Error::InvalidState`] if too many lookups are already in flight.
    ///   Transient: retry once earlier ones drain.
    pub fn resolve_username(&mut self, username: &str) -> Result<bool> {
        let username: Username = username
            .parse()
            .map_err(|e| Error::InvalidArgument(format!("Not a resolvable username: {}", e)))?;

        if !self.transport_manager.nostr_discovery_active() {
            return Err(Error::InvalidConfiguration(
                "Username discovery is disabled; enable nostr_username_discovery_enabled \
                 (which also requires nostr_cold_contact_enabled)"
                    .to_string(),
            ));
        }

        // Deduplicated against both halves of "in flight", because the transport
        // can only see the first. A lookup lives in its queue until the platform
        // mints a query, and from then on it exists solely as a resolution here.
        // Checking only the transport would let the second request for a name
        // mint a duplicate REQ and emit a second event for one lookup, breaking
        // the exactly-one-event contract the whole API shape rests on.
        if self.nostr_resolution_requests.contains_key(&username)
            || self
                .nostr_resolutions
                .values()
                .any(|resolution| resolution.username == username)
        {
            return Ok(false);
        }

        match self
            .transport_manager
            .resolve_nostr_username(username.clone())
        {
            ResolveRequest::Queued => {}
            // Reachable despite the check above only if the two fell out of
            // step; the in-flight lookup still answers this caller.
            ResolveRequest::AlreadyQueued => return Ok(false),
            ResolveRequest::Disabled => {
                return Err(Error::InvalidConfiguration(
                    "Username discovery is disabled".to_string(),
                ))
            }
            ResolveRequest::QueueFull => {
                return Err(Error::InvalidState(
                    "Too many username lookups in flight; retry shortly".to_string(),
                ))
            }
        }

        // The deadline starts here, not at query mint, because minting is what
        // may never happen: the platform pumps queries only while the relay
        // socket is up, so a lookup requested offline sits in the transport
        // queue with no resolution behind it and no clock running. Timing from
        // the request is what makes "a lookup was started" a promise this
        // engine can keep.
        self.nostr_resolution_requests
            .entry(username)
            .or_insert_with(Instant::now);
        Ok(true)
    }

    /// Registers a query the transport just minted, so its answers accumulate.
    ///
    /// A mint whose request has already been answered by the timeout sweep is
    /// dropped rather than accumulated. That is what keeps the one-event
    /// contract when a relay reconnects after the sweep gave up: the query
    /// still goes out, its answers land on an unknown resolution and are
    /// discarded, and no second, contradictory set is emitted for a lookup the
    /// app has already been told the answer to.
    pub fn begin_username_resolution(&mut self, query_id: String, username: Username) {
        if self.nostr_resolution_requests.remove(&username).is_none() {
            debug!("Discovery query minted for an already-answered request; not accumulating");
            return;
        }

        if self.nostr_resolutions.len() >= MAX_CONCURRENT_RESOLUTIONS {
            // Flush the oldest rather than refusing the newest: its caller is
            // still owed an answer, and an answer with fewer claims beats a
            // silence that never resolves.
            if let Some(stale) = self
                .nostr_resolutions
                .iter()
                .min_by_key(|(_, resolution)| resolution.started)
                .map(|(id, _)| id.clone())
            {
                self.flush_username_resolution(&stale);
            }
        }

        // Two live resolutions share an id only when the transport's RNG failed
        // and every query fell back to one fixed id. Flushing the incumbent
        // keeps the promise made to *its* caller: overwriting it silently would
        // leave that lookup waiting forever, because the request record it
        // would have been swept from is already consumed.
        if self.nostr_resolutions.contains_key(&query_id) {
            warn!(
                query_id = %query_id,
                "Discovery query id collision; flushing the resolution it displaces"
            );
            self.flush_username_resolution(&query_id);
        }

        self.nostr_resolutions.insert(
            query_id,
            PendingResolution {
                username,
                claims: HashMap::new(),
                tombstoned: HashSet::new(),
                rejected: 0,
                truncated: 0,
                started: Instant::now(),
            },
        );
    }

    /// Accumulates one discovery record returned by a resolution query.
    ///
    /// Verification happens here and every failure is *ordinary*: the tag is
    /// public, anyone may publish to it, and a query returns whatever the relay
    /// holds. A refused record is counted and dropped, never surfaced as an
    /// error — a resolver that reported junk as a failure would make an
    /// unremarkable directory look broken.
    /// The name a record is checked against is the *resolution's* own, not one
    /// passed in beside the query id. The two agree today because the transport
    /// hands back the subject it stored, but taking both would let a caller
    /// seed one query's claim set with records verified for another name, and
    /// the username check is what catches a genuine record copied onto a
    /// foreign tag.
    pub fn handle_resolved_discovery_record(&mut self, query_id: &str, author: &str, data: &[u8]) {
        let Some(resolution) = self.nostr_resolutions.get_mut(query_id) else {
            debug!(query_id = %query_id, "Discovery record for an unknown resolution");
            return;
        };

        let body = match parse_discovery_body(data) {
            Ok(body) => body,
            Err(e) => {
                debug!(error = %e, "Undecodable discovery record");
                resolution.rejected = resolution.rejected.saturating_add(1);
                return;
            }
        };

        let record = match body {
            DiscoveryBody::Record(record) => *record,
            DiscoveryBody::Tombstone => {
                // A retraction. Drop any claim this author had made and refuse
                // any that arrives later: the tombstone replaced their record
                // at the relay, so seeing both means we read a stale copy from
                // one relay and the retraction from another. The retraction is
                // the newer statement whichever order they land in.
                resolution.claims.remove(author);
                resolution.tombstoned.insert(author.to_string());
                return;
            }
        };

        // Checked before the signature verify, both because it is cheaper and
        // because a retracted author's record is refused on the strength of the
        // retraction rather than on anything about the record.
        if resolution.tombstoned.contains(author) {
            debug!("Discovery record from an author that has retracted; ignoring");
            return;
        }

        let author_bytes = match hex::decode(author) {
            Ok(bytes) => bytes,
            Err(_) => {
                resolution.rejected = resolution.rejected.saturating_add(1);
                return;
            }
        };

        if let Err(rejection) =
            verify_discovery_record(&record, &resolution.username, &author_bytes)
        {
            debug!(reason = %rejection, "Discovery record refused");
            resolution.rejected = resolution.rejected.saturating_add(1);
            return;
        }

        if resolution.claims.len() >= MAX_CLAIMS_PER_RESOLUTION
            && !resolution.claims.contains_key(author)
        {
            debug!(
                cap = MAX_CLAIMS_PER_RESOLUTION,
                "Username resolution at its claim ceiling; ignoring the rest"
            );
            resolution.truncated = resolution.truncated.saturating_add(1);
            return;
        }

        // One device, one claim: a repeat from the same author is that device
        // republishing, so the newer statement wins.
        match resolution.claims.get(author) {
            Some(existing) if existing.issued_at_ms >= record.issued_at_ms => {}
            _ => {
                resolution.claims.insert(author.to_string(), record);
            }
        }
    }

    /// Emits the accumulated set for a finished resolution.
    ///
    /// Idempotent: a resolution already flushed (by the timeout sweep, say) is
    /// simply absent, so a late end-of-stored-events emits nothing rather than
    /// a second, contradictory set.
    pub fn flush_username_resolution(&mut self, query_id: &str) {
        let Some(resolution) = self.nostr_resolutions.remove(query_id) else {
            return;
        };

        let claims: Vec<UsernameClaim> = resolution
            .claims
            .into_values()
            .map(|record| UsernameClaim {
                address: record.address.to_string(),
                public_key: base64::engine::general_purpose::STANDARD.encode(&record.pubkey),
                issued_at_ms: record.issued_at_ms,
            })
            .collect();

        self.emit_event(Event::UsernameResolved {
            username: resolution.username.into_string(),
            claims,
            rejected: resolution.rejected,
            truncated: resolution.truncated,
        });
    }

    /// Flushes resolutions whose relays never sent end-of-stored-events, and
    /// answers lookups whose query was never minted at all.
    ///
    /// Runs on the process tick. Two distinct hangs, one deadline:
    ///
    /// - a relay that answers and then goes quiet leaves a resolution
    ///   accumulating with no completion signal, since end-of-stored-events is
    ///   the only one a Nostr query has;
    /// - a lookup requested while the relay socket is down never reaches
    ///   [`Self::begin_username_resolution`] at all, because the platform pumps
    ///   queries only while connected. Nothing is accumulating, so there is
    ///   nothing for the first sweep to find, and the app waits forever on an
    ///   event with no trigger.
    ///
    /// The second case also **cancels** the queued lookup. Leaving it in the
    /// transport queue would make every later `resolve_username` for that name
    /// return `false` ("already queued") without ever emitting, so a name that
    /// timed out once could never be looked up again for the life of the
    /// process.
    pub(crate) fn sweep_username_resolutions(&mut self) {
        let now = Instant::now();

        if !self.nostr_resolutions.is_empty() {
            let expired: Vec<String> = self
                .nostr_resolutions
                .iter()
                .filter(|(_, resolution)| {
                    now.duration_since(resolution.started) > RESOLUTION_TIMEOUT
                })
                .map(|(id, _)| id.clone())
                .collect();

            for query_id in expired {
                debug!(query_id = %query_id, "Username resolution timed out; emitting what it has");
                self.flush_username_resolution(&query_id);
            }
        }

        if self.nostr_resolution_requests.is_empty() {
            return;
        }

        let unminted: Vec<Username> = self
            .nostr_resolution_requests
            .iter()
            .filter(|(_, requested_at)| now.duration_since(**requested_at) > RESOLUTION_TIMEOUT)
            .map(|(username, _)| username.clone())
            .collect();

        for username in unminted {
            debug!("Username lookup never reached a relay; emitting an empty answer");
            self.nostr_resolution_requests.remove(&username);
            self.transport_manager
                .cancel_nostr_username_resolution(&username);
            // An empty set, on the same terms as a query that reached a relay
            // and found nothing. That is already what this engine emits when
            // the platform releases a query with no relay connected, so the
            // two unreachable paths report identically rather than one of them
            // being silent.
            self.emit_event(Event::UsernameResolved {
                username: username.into_string(),
                claims: Vec::new(),
                rejected: 0,
                truncated: 0,
            });
        }
    }

    /// Persists which username this install claims, and which it owes a
    /// tombstone for.
    pub(crate) fn persist_nostr_discovery_claim(&mut self) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        let state = DiscoveryClaimState {
            claim: self.nostr_discovery_claim.clone(),
            retracting: self.nostr_discovery_retracting.clone(),
        };
        let bytes = match serde_json::to_vec(&state) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "Failed to serialize the Nostr discovery claim");
                return;
            }
        };
        if let Err(e) = self.write_state_record(
            storage.as_ref(),
            storage_keys::NOSTR_DISCOVERY_CLAIM,
            storage_keys::NOSTR_DISCOVERY_CLAIM_ID,
            &bytes,
        ) {
            warn!(error = %e, "Failed to persist the Nostr discovery claim");
        }
    }

    /// Restores the standing claim and any retraction still owed.
    ///
    /// Every failure lands on an empty state, which means the next tick
    /// publishes the current profile's claim and *does not* retract whatever
    /// was standing. That is the benign direction: guessing at a name to
    /// retract would publish a tombstone at a tag this install may never have
    /// claimed.
    ///
    /// Reads the pre-retraction shape too, which was a bare `Option<Username>`.
    /// The two are distinguishable on sight (an object against a string or
    /// `null`) and an install that upgrades mid-life would otherwise lose its
    /// standing claim and republish it under a second addressable slot.
    pub(crate) fn restore_nostr_discovery_claim(&mut self) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };

        let data = match self.read_state_record(
            storage.as_ref(),
            storage_keys::NOSTR_DISCOVERY_CLAIM,
            storage_keys::NOSTR_DISCOVERY_CLAIM_ID,
        ) {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(e) => {
                warn!(error = %e, "Failed to read the Nostr discovery claim; starting fresh");
                return;
            }
        };

        let restored = serde_json::from_slice::<DiscoveryClaimState>(&data).or_else(|_| {
            serde_json::from_slice::<Option<Username>>(&data).map(|claim| DiscoveryClaimState {
                claim,
                retracting: Vec::new(),
            })
        });

        match restored {
            Ok(state) => {
                self.nostr_discovery_claim = state.claim;
                self.nostr_discovery_retracting = state.retracting;
                // A restored claim has not been published *this* process, and
                // an addressable record lives on the relays rather than here.
                self.nostr_discovery_published = false;
            }
            Err(e) => warn!(error = %e, "Corrupted Nostr discovery claim; starting fresh"),
        }
    }
}
