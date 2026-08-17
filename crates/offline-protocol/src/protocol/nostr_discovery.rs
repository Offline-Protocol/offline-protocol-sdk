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
const MAX_PENDING_RESOLUTIONS: usize = 32;

/// Maximum claims accumulated for one username.
///
/// The tag is public, so a squatter can publish many records at it. This caps
/// what one resolution can cost in memory and what an app is asked to render.
/// Reaching it means crowding, which the design accepts: the honest claimants
/// may be among the ones displaced, which is why the invite path exists and
/// why a user confirms out of band.
const MAX_CLAIMS_PER_RESOLUTION: usize = 32;

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
    /// 4. discovery is on and the claim is current: republish once per process.
    ///
    /// State 3 is the one that is easy to omit and expensive to omit. A user
    /// who renames leaves their old name standing in a public directory,
    /// pointing at an address that is still live, with nothing to ever remove
    /// it — and the *new* owner of that name looks like a squatter next to it.
    pub(crate) fn refresh_nostr_discovery_claim(&mut self) {
        if !self.nostr_discovery_refresh_due() {
            return;
        }

        let now = Instant::now();

        // Drained after the throttle check, like the slot reports: draining is
        // destructive and a throttled tick that dropped them would lose them.
        for tag in self
            .transport_manager
            .take_failed_nostr_discovery_publications()
        {
            self.nostr_discovery_published = false;
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

        // Retract whatever stands that should not.
        if let Some(standing) = self.nostr_discovery_claim.clone() {
            if desired.as_ref() != Some(&standing) {
                self.retract_discovery_claim(standing);
            }
        }

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
        if self.nostr_discovery_claim.as_ref() != Some(&username) {
            self.nostr_discovery_claim = Some(username);
            self.persist_nostr_discovery_claim();
        }
        Ok(())
    }

    /// Queues a tombstone and a best-effort deletion for a standing claim.
    ///
    /// The persisted record is cleared *before* the queue call rather than
    /// after: a retraction that fails to reach a relay leaves a claim we no
    /// longer track, which is the recoverable direction (the claim expires when
    /// its second hop fails). Keeping the record and failing to clear it would
    /// instead have the next tick retract again forever.
    fn retract_discovery_claim(&mut self, username: Username) {
        match serde_json::to_vec(&DiscoveryTombstoneV1::new()) {
            Ok(payload) => {
                self.transport_manager
                    .retract_nostr_discovery_record(username, payload);
            }
            Err(e) => warn!(error = %e, "Failed to build a discovery tombstone"),
        }
        self.nostr_discovery_claim = None;
        self.nostr_discovery_published = false;
        self.persist_nostr_discovery_claim();
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
    /// Returns whether a query was queued. `false` means discovery is off, the
    /// name is already being resolved, or the queue is full — all of which are
    /// ordinary, and none of which an app needs to distinguish: the resolution
    /// already in flight will emit for both callers.
    pub fn resolve_username(&mut self, username: &str) -> Result<bool> {
        let username: Username = username
            .parse()
            .map_err(|e| Error::InvalidArgument(format!("Not a resolvable username: {}", e)))?;

        if !self.transport_manager.nostr_discovery_active() {
            return Ok(false);
        }

        if !self
            .transport_manager
            .resolve_nostr_username(username.clone())
        {
            return Ok(false);
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

        if self.nostr_resolutions.len() >= MAX_PENDING_RESOLUTIONS {
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

        self.nostr_resolutions.insert(
            query_id,
            PendingResolution {
                username,
                claims: HashMap::new(),
                tombstoned: HashSet::new(),
                rejected: 0,
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
    pub fn handle_resolved_discovery_record(
        &mut self,
        query_id: &str,
        username: &Username,
        author: &str,
        data: &[u8],
    ) {
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

        if let Err(rejection) = verify_discovery_record(&record, username, &author_bytes) {
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
            });
        }
    }

    /// Persists which username this install currently claims.
    pub(crate) fn persist_nostr_discovery_claim(&mut self) {
        let Some(storage) = self.protocol_state_storage.clone() else {
            return;
        };
        let bytes = match serde_json::to_vec(&self.nostr_discovery_claim) {
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

    /// Restores the standing claim.
    ///
    /// Every failure lands on `None`, which means the next tick publishes the
    /// current profile's claim and *does not* retract whatever was standing.
    /// That is the benign direction: an unretracted claim expires when its
    /// second hop fails, whereas guessing at a name to retract would publish a
    /// tombstone at a tag this install may never have claimed.
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

        match serde_json::from_slice::<Option<Username>>(&data) {
            Ok(claim) => {
                self.nostr_discovery_claim = claim;
                // A restored claim has not been published *this* process, and
                // an addressable record lives on the relays rather than here.
                self.nostr_discovery_published = false;
            }
            Err(e) => warn!(error = %e, "Corrupted Nostr discovery claim; starting fresh"),
        }
    }
}
