//! Replicating documents between two peers over the ordinary message ladder.
//!
//! # Why this is small
//!
//! Almost everything replication needs is already here. Deltas are
//! idempotent and commutative, so at-least-once, unordered, partition
//! tolerant delivery is not a compromise this layer works around: it is
//! exactly enough, by construction. That means no ordering, no sequence
//! numbers, no session streams, and no second delivery path. A sync frame is
//! a message, and every reliability property the message ladder has spent
//! ten releases acquiring becomes a sync property for free.
//!
//! # The space is the session
//!
//! A space replicates with the peer whose address names it. Inbound frames
//! take the space from the authenticated wire sender, outbound frames from
//! the recipient, and neither ever reads a space name off the wire. So a
//! frame from one peer cannot touch another peer's documents, and there is
//! no authorization table to get wrong. The two replicas name the same space
//! differently — each by the *other's* address — which is why the name is
//! absent from the frames.
//!
//! # Anti-entropy, not a protocol
//!
//! On session confirm, on reconnect, and at startup, each side offers its
//! version of every document it holds for that peer. The other side answers
//! with whatever is missing and its own versions, and the exchange stops.
//! Nothing persists about how far a peer has got: state that has to be
//! reconciled after a crash is state that can be wrong, and the version
//! exchange already answers the only question that matters.
//!
//! # Every leg ends
//!
//! An inbound frame produces at most one kind of outbound answer, and every
//! chain of answers is finite: an offer provokes one reply, a delta that
//! cannot apply provokes one request, and the snapshot that request is
//! answered with provokes nothing at all. The one chain longer than a single
//! hop is a reply naming a document this device has never seen: it is asked
//! for, and the answer is the document. That ends too, because the question
//! names only documents the peer has just offered, so the peer creates
//! nothing from it and has nothing further to ask. That is the property to
//! preserve when adding a frame kind. Two replicas that answer each other's
//! answers converge perfectly well and then talk until the battery dies, and
//! the failure has no symptom on either device except traffic.
//!
//! One outbound frame is not an answer and does not count against this: a
//! blob arriving for a document with local edits still pending flushes them
//! first, which pushes them. That is work this device already owed the peer
//! and the frame merely reached it, so it cannot recur: what it sends is
//! what was pending before the frame arrived, and flushing is what stops it
//! being pending. When that flush fails and the import then applies, the
//! edit may have been folded into the imported change and suppressed with
//! it, so a version offer is sent instead of the delta. That one is not an
//! answer either, and it cannot recur for the same reason: it costs a
//! storage failure that recovered inside a single frame.
//!
//! # Remote bytes are not our bytes
//!
//! Everywhere else in the data layer, the argument for handing bytes to the
//! document engine is that they came back out of a sealed record whose AEAD
//! tag already vouched for them. That argument does not survive contact with
//! a network. MLS says who sent a blob; it says nothing about its shape, and
//! the engine has an open defect where a malformed one aborts the process
//! rather than returning an error. So blobs arriving here are judged before
//! the engine sees them, and the one that gets through is remembered on disk
//! until it has survived, so a blob that kills the process kills it once
//! rather than every time the sender retries.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use offline_protocol_data::{CatchUp, RemoteImport, VersionToken};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::events::Event;
use crate::media_envelope::DataPurpose;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::types::{storage_keys, MediaSendOptions, DATA_SYNC_V1};
use crate::protocol::OfflineProtocol;
use offline_protocol_core::ContentType;

/// Largest document blob carried inside one sync frame, before base64.
///
/// Sized to the mesh rather than to the record store. An unnegotiated
/// Bluetooth link fragments a message into at most 512 pieces of 139 usable
/// bytes, so roughly 69 KiB is the hard ceiling for a single message on the
/// worst transport we ship; 32 KiB leaves room for base64 expansion, the
/// frame's own JSON, the sealed envelope and the message header, and still
/// arrives in one piece. Anything larger is not refused forever, it is
/// answered by a snapshot offer that the media path will carry.
pub(crate) const MAX_SYNC_BLOB_BYTES: usize = 32 * 1024;

/// Documents listed in one version frame.
///
/// A name is capped at 128 bytes and an encoded version is short, so this
/// keeps a version frame comfortably inside [`MAX_SYNC_BLOB_BYTES`] without
/// having to measure it. A space with more documents than this sends
/// several frames, which converges identically: the exchange is per
/// document, not per frame.
const MAX_DOCS_PER_VERSION_FRAME: usize = 128;

/// How long an offer to one peer suppresses the next one.
///
/// Sized to discovery flapping rather than to editing: a change made locally
/// is pushed the moment it is durable and does not wait for this, so the only
/// thing the window delays is the reconciliation sweep, and the next trigger
/// repeats it anyway.
const DATA_SYNC_OFFER_INTERVAL: Duration = Duration::from_secs(30);

/// Documents one space may hold on a peer's say-so.
///
/// Every name in an offer that this device does not recognise becomes a
/// stored document, and nothing else bounds how many names one exchange can
/// carry. The ceiling is far above what two people sharing documents reach
/// and far below what a peer streaming fresh names could otherwise spend, so
/// it is a bound on abuse rather than a product limit. It applies only to
/// documents a peer names: an application creating its own is not restricted
/// by it, and those still replicate outward.
pub(crate) const MAX_DOCS_PER_SPACE: usize = 1024;

/// Blob digests remembered per space after a crash.
///
/// Small on purpose. The list only has to outlive the sender's retries of
/// one blob, and every entry in it is a document change we have decided
/// never to apply, so a long list is a liability rather than a safety net.
pub(crate) const MAX_QUARANTINED_BLOBS: usize = 32;

/// Largest document encoding this device will carry over the media path.
///
/// The record ceiling, because that is the real limit: a document larger
/// than one sealed protocol-state record cannot be persisted by the receiver
/// even if every byte arrives, so carrying it would spend a long transfer to
/// fail at the end. Well above the 1 MiB compacted cap a document is held
/// to, which leaves room for the history a raw snapshot carries.
pub(crate) const MAX_MEDIA_SNAPSHOT_BYTES: usize = super::types::MAX_PROTOCOL_STATE_RECORD_BYTES;

/// Attachment fetches one device may have outstanding at once.
///
/// Bounded because each entry is created by a local call and cleared by a
/// remote answer that may never come. Passing the bound evicts the oldest:
/// a fetch that has been waiting longer than sixty-three others is not the
/// one still worth remembering.
pub(crate) const MAX_PENDING_ATTACHMENT_FETCHES: usize = 64;

/// How long a fetch stays outstanding before it is given up on.
///
/// Generous, because the answer travels the media path and the media path
/// may be Bluetooth. The cost of being slow here is a reference that reports
/// unavailable while its bytes are still arriving, and the app can ask
/// again; the cost of no timeout at all is a map that only grows.
pub(crate) const ATTACHMENT_FETCH_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// One sync frame's body, inside the decrypted MLS plaintext.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k")]
enum SyncBody {
    /// "Here is what I have; send me what I am missing."
    ///
    /// `reply` marks the answering leg, which is what stops the exchange:
    /// an offer provokes an answer, and an answer provokes nothing.
    #[serde(rename = "vv")]
    Versions {
        #[serde(default)]
        reply: bool,
        /// Whether this frame carries less than everything the sender holds:
        /// one frame of an offer split across several, or a single document
        /// named because something about it needs answering.
        ///
        /// It exists because a receiver reads a document's *absence* from an
        /// offer as "they have never seen this", and answers with the whole
        /// document. That inference needs the complete list. Drawn from one
        /// frame of a split offer it fires on every document the frame did
        /// not happen to carry, so a peer holding more documents than one
        /// frame fits would be sent the entire space, in full, on every
        /// exchange, while perfectly in sync.
        #[serde(default)]
        partial: bool,
        /// Document name to base64 of its version token.
        docs: BTreeMap<String, String>,
    },
    /// A run of changes, base64.
    #[serde(rename = "delta")]
    Delta { doc: String, blob: String },
    /// A whole document, base64. The answer to a gap no run of changes can
    /// express.
    #[serde(rename = "snap")]
    Snapshot { doc: String, blob: String },
    /// "No run of changes can close my gap in this document; send the whole
    /// thing."
    ///
    /// The frame that makes a refusal recoverable. Without it a receiver
    /// that declines a delta for reaching below its trim point has no way to
    /// say what would work: answering with a version offer instead asks the
    /// sender to compute changes since our version, which is the same
    /// refused delta again, for as long as both sides keep at it.
    #[serde(rename = "need_snap")]
    NeedSnapshot { doc: String },
    /// "I hold a reference to these bytes and not the bytes; do you have
    /// them?"
    ///
    /// Carries no document name. A blob is addressed by what it is rather
    /// than by where a reference to it happens to sit, so the same bytes
    /// referenced from three documents are one blob and one request.
    #[serde(rename = "need_blob")]
    NeedBlob { hash: String },
    /// "I do not have those bytes, and asking again will not change that."
    ///
    /// The frame that lets a fetch end. An attachment reference outlives the
    /// bytes it names, because the reference replicates and the bytes do
    /// not, so a peer holding a reference and no blob is ordinary rather
    /// than broken. Without an answer for it the asking side cannot tell
    /// that case from a peer that is merely slow, and shows a spinner
    /// forever.
    #[serde(rename = "blob_gone")]
    BlobGone { hash: String },
}

/// Where a sync frame goes and what it is sealed under.
///
/// The one thing F4 added to this module. 1:1 replication could treat the
/// space, the peer, and the reply address as a single value because they
/// genuinely were one; a group space is one scope with many members, so the
/// space no longer names the recipient and the two have to be carried
/// separately.
#[derive(Debug, Clone)]
pub(crate) enum SyncChannel {
    /// A 1:1 space, sealed to the peer the space is named after.
    Peer,
    /// A group space, sealed once for the whole roster.
    ///
    /// Used only for changes every member needs: a local commit. Group MLS
    /// produces one ciphertext for everyone, which is what makes group
    /// replication cheap, and it is also why this must never be used for an
    /// answer to one member's question — the other members would each
    /// receive, decrypt, and import a blob they already had.
    GroupBroadcast,
    /// A group space, sealed for the roster but delivered to one member.
    ///
    /// The reply leg. Anti-entropy between two members of a group is still
    /// a conversation between two devices: an offer is answered by the
    /// member that received it and by nobody else. Every other member is
    /// spared the traffic, and the terminating 1.5-round-trip exchange the
    /// 1:1 path relies on keeps working unchanged.
    ///
    /// Sealed under the group key rather than the pair's 1:1 session on
    /// purpose: two members of a group need not have a 1:1 session at all,
    /// and requiring one would make replication depend on a handshake that
    /// may never happen.
    GroupDirected(String),
}

/// Which frame a blob arrived in.
///
/// Carried into the import so a rung of the catch-up ladder knows whether
/// there is a rung above it. A delta that cannot apply has one; a snapshot
/// is already the top, and asking again returns the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlobKind {
    Delta,
    Snapshot,
}

/// Per-space replication bookkeeping.
///
/// Everything here is about surviving a crash. Nothing in it is needed for
/// convergence, which is deliberate: the version exchange is the recovery
/// mechanism, so this record can be lost without costing a change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SyncRecord {
    /// The blob currently being handed to the document engine.
    ///
    /// Written before the engine is called and cleared after it returns. A
    /// value still here at open means the last process to touch this space
    /// did not survive that call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_flight: Option<String>,
    /// Blobs that were in flight when a previous run died, newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    quarantined: Vec<String>,
}

/// The digest a blob is remembered by.
pub(crate) fn blob_digest(blob: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(blob);
    // Half a SHA-256 is 128 bits, which is far past what a collision would
    // need to be worth engineering: the prize is having one specific blob
    // refused on one device.
    hex_encode(&hasher.finalize()[..16])
}

/// The rate-limit key for offers toward one member of one group.
///
/// A group space and a 1:1 space share one window map, so the two key
/// spaces must not overlap: a bare peer address is the 1:1 key, and this
/// one is deliberately built with a character no validated peer id, group
/// id or address can contain, so no composed key can ever collide with a
/// peer's own.
fn group_offer_key(member: &str, group_id: &str) -> String {
    format!("{member}{GROUP_OFFER_KEY_SEP}{group_id}")
}

/// The separator [`group_offer_key`] composes with, named so the split that
/// takes a key back apart cannot drift from the format that built it.
const GROUP_OFFER_KEY_SEP: char = '\x01';

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl OfflineProtocol {
    // ---- outbound ---------------------------------------------------

    /// Offer our versions of every document shared with `peer`.
    ///
    /// The entry point for all three triggers (session confirm, reconnect,
    /// startup). Cheap and safe to call spuriously: a peer that is already
    /// current answers with nothing.
    pub(crate) fn kick_data_sync(&mut self, peer: &str, cause: &str) {
        if !self.data_sync_active(peer) {
            return;
        }
        if !self.data_sync_offer_due(peer) {
            return;
        }
        self.offer_versions(peer, cause);
    }

    /// Offer our versions of every document in `group_id` to one member.
    ///
    /// The group counterpart of [`Self::kick_data_sync`], and addressed for
    /// the same reason its answers are: a member coming back into range is
    /// a conversation with that member. Broadcasting the offer to the whole
    /// roster would have every other member answer a question nobody asked
    /// them.
    ///
    /// Rate-limited per (member, group) rather than per member, so a peer
    /// shared across several groups reconciles all of them rather than
    /// whichever one its rediscovery happened to reach first.
    #[cfg_attr(not(feature = "data"), allow(dead_code))]
    pub(crate) fn kick_group_data_sync(&mut self, group_id: &str, member: &str, cause: &str) {
        let members = match self.group_roster(group_id) {
            Ok(members) => members,
            Err(_) => return,
        };
        if !self.group_data_sync_active(&members) {
            // Rediscovery is the one trigger a group space has that does
            // not require somebody to be editing, so it is also the only
            // moment a device that never writes anything will notice the
            // gate is shut. Probing only from the local-commit path would
            // leave a member that merely reads waiting for someone else to
            // do it.
            self.probe_group_data_capabilities(group_id, &members);
            return;
        }
        let key = group_offer_key(member, group_id);
        if !self.data_sync_offer_due(&key) {
            return;
        }
        self.offer_versions_over(
            group_id,
            &SyncChannel::GroupDirected(member.to_string()),
            cause,
        );
    }

    /// Ask the members holding a group's replication gate closed what they
    /// support.
    ///
    /// For a group the usual reason the gate is shut is not that somebody
    /// opted out but that we have never heard from them: members added by a
    /// third party exchange no key packages with us, and an inviter running
    /// an SDK from before attestation forwards none. Sending them ours
    /// makes their automatic reply teach us what they support.
    ///
    /// The probe itself is capability-agnostic and guarded by
    /// `key_package_sent_to`, so a group that stays closed does not re-probe
    /// on every commit or every rediscovery.
    #[cfg_attr(not(feature = "data"), allow(dead_code))]
    fn probe_group_data_capabilities(&mut self, space: &str, members: &[String]) {
        let unknown = self.group_data_unknown_members(members);
        if unknown.is_empty() {
            return;
        }
        debug!(
            space,
            unknown = unknown.len(),
            "Group replication is held closed by members of unknown capability; probing"
        );
        self.backfill_group_rich_capabilities(&unknown);
    }

    /// Drop every offer window belonging to `peer`: the 1:1 one, and the
    /// group one for each group shared with them.
    ///
    /// The group windows are keyed by (member, group), so removing the bare
    /// peer key leaves them all behind, and each of them suppresses the
    /// first group offer to that peer for a further window once the
    /// capability is relearned. That is the same silent non-sync
    /// [`Self::forget_data_sync_peer`] exists to prevent, one space at a
    /// time.
    ///
    /// Windows left by a group this device *left* are deliberately not
    /// swept here: they name a scope that no longer replicates at all, and
    /// on a re-join the worst they can cost is one suppressed offer that
    /// the next discovery re-sends.
    #[cfg(feature = "data")]
    pub(crate) fn forget_data_sync_offer_windows(&mut self, peer: &str) {
        self.last_data_sync_offer.retain(|key, _| {
            key != peer
                && !key
                    .split_once(GROUP_OFFER_KEY_SEP)
                    .is_some_and(|(member, _)| member == peer)
        });
    }

    /// Whether the reconciliation sweep keyed by `key` is outside its
    /// suppression window, stamping it when it is.
    ///
    /// Stamped here rather than after the send for the reason
    /// [`Self::offer_versions_over`] gives: a read that fails will still
    /// fail a moment later, and retrying it at discovery speed is the
    /// traffic the window exists to prevent.
    fn data_sync_offer_due(&mut self, key: &str) -> bool {
        if let Some(last) = self.last_data_sync_offer.get(key) {
            if Instant::now().duration_since(*last) < DATA_SYNC_OFFER_INTERVAL {
                return false;
            }
        }
        self.last_data_sync_offer
            .insert(key.to_string(), Instant::now());
        true
    }

    /// Offer versions of every group space this device shares with `peer`.
    ///
    /// Wired to the same rediscovery seam as [`Self::kick_data_sync`], and
    /// needed because a group space has no 1:1 trigger to ride: two members
    /// may never establish a session with each other, so without this the
    /// only thing that ever reconciles them is a fresh local commit.
    ///
    /// Which groups this device is in comes from MLS, not from the roster
    /// cache alone. The cache is filled on demand and is empty after a
    /// restart, so reading only it would make this sweep silently do nothing
    /// on a cold process, which is the launch where it matters most: two
    /// members that drifted apart and are not editing have no other trigger
    /// left.
    #[cfg_attr(not(feature = "data"), allow(dead_code))]
    pub(crate) fn kick_shared_group_data_sync(&mut self, peer: &str, cause: &str) {
        if !self.config.data.enabled {
            return;
        }
        self.enumerate_group_spaces_once();
        let shared: Vec<String> = self
            .group_mesh
            .members
            .iter()
            .filter(|(_, members)| members.iter().any(|m| m == peer))
            .map(|(group_id, _)| group_id.clone())
            .collect();
        for group_id in shared {
            self.kick_group_data_sync(&group_id, peer, cause);
        }
    }

    /// Fill the roster cache from MLS once per session, so
    /// [`Self::kick_shared_group_data_sync`] can see a group that nothing has
    /// touched yet.
    ///
    /// Costs one key listing, plus a group-state read for each group not
    /// already cached — the same read the next send to that group would have
    /// paid. Marked done even when a group read fails: the failure is per
    /// group and retrying the whole enumeration on every discovery is the
    /// traffic the offer window exists to prevent. An MLS that is not ready
    /// yet is not a failure of this kind and leaves the flag alone, so the
    /// next discovery tries again.
    fn enumerate_group_spaces_once(&mut self) {
        if self.group_spaces_enumerated {
            return;
        }
        let Ok(groups) = self.list_groups() else {
            return;
        };
        self.group_spaces_enumerated = true;
        for group_id in groups {
            if self.group_mesh.members.contains_key(&group_id) {
                continue;
            }
            if let Err(err) = self.refresh_group_members(&group_id) {
                debug!(group_id = %group_id, error = %err, "Group roster unreadable while enumerating group spaces");
            }
        }
    }

    /// Offer our versions because a change happened that no frame carried,
    /// without waiting for the rate limit.
    ///
    /// `origin` names the peer a change arrived from, so one applied from
    /// the very peer this space replicates with is not announced back.
    ///
    /// The limiter damps discovery flapping, where the offer it suppresses
    /// is a sweep the next trigger repeats anyway. Nothing repeats this one.
    /// A change too large to inline left no trace on the wire at all, so
    /// until somebody asks, both sides hold documents they believe agree;
    /// on a link that never drops there is no next trigger to ask.
    pub(crate) fn nudge_data_sync(&mut self, space: &str, origin: Option<&str>, cause: &str) {
        match self.space_channel(space, origin) {
            Some(SyncChannel::Peer) => self.offer_versions(space, cause),
            // The roster-wide leg, and the one place a group offer is not
            // addressed to one member. What went unsent was a change every
            // member needs, and there is no single member to ask: each of
            // them answers with what they are missing, and a member that is
            // already current answers with nothing.
            Some(channel @ SyncChannel::GroupBroadcast) => {
                self.offer_versions_over(space, &channel, cause)
            }
            Some(SyncChannel::GroupDirected(_)) | None => {}
        }
    }

    /// How a space replicates, or `None` when it does not.
    ///
    /// The single place the three kinds of space are told apart. A space is
    /// a group space when MLS says this device holds a group by that name,
    /// a 1:1 space when the name is a peer that advertised replication, and
    /// local-only otherwise — and "local-only" is the honest answer for a
    /// space whose peer never advertised, whose group is gone, or whose
    /// members are not all group-capable.
    ///
    /// `origin`, when set, names the peer a change was just applied from,
    /// and suppresses announcing that change back to them.
    fn space_channel(&mut self, space: &str, origin: Option<&str>) -> Option<SyncChannel> {
        if let Some(members) = self.group_space_roster(space) {
            // A change that arrived over the group reached every member at
            // once, because that is what one group ciphertext does. Pushing
            // it on would have every member push every change to every
            // other member: N times the frames for nothing, and worse as
            // the group grows. Anti-entropy still closes real gaps.
            if origin.is_some() {
                return None;
            }
            if self.group_data_sync_active(&members) {
                return Some(SyncChannel::GroupBroadcast);
            }
            self.probe_group_data_capabilities(space, &members);
            return None;
        }
        if origin == Some(space) || !self.data_sync_active(space) {
            return None;
        }
        Some(SyncChannel::Peer)
    }

    /// Send our version of every document in a space to the peer that names
    /// it, and start the window that suppresses the next sweep.
    fn offer_versions(&mut self, peer: &str, cause: &str) {
        // Stamped even on the unlimited path so an offer sent right now
        // still suppresses the next sweep: the sweep would re-send what
        // this frame already carried.
        self.last_data_sync_offer
            .insert(peer.to_string(), Instant::now());
        self.offer_versions_over(peer, &SyncChannel::Peer, cause);
    }

    /// [`Self::offer_versions`] over an explicit channel, which is what a
    /// group space needs: the offer is addressed to one member, not
    /// broadcast to the roster.
    fn offer_versions_over(&mut self, space: &str, channel: &SyncChannel, cause: &str) {
        let peer = space;
        let docs = match self.data_sync_versions(peer) {
            Ok(docs) => docs,
            Err(err) => {
                // A whole space that cannot enumerate its documents stops
                // replicating with this peer until the storage recovers, and
                // nothing else reports it. The rate limit above is stamped
                // before this read precisely so warning here cannot become
                // discovery-speed noise.
                warn!(peer = %peer, cause, error = %err, "Could not read document versions");
                return;
            }
        };
        debug!(space = %peer, cause, docs = docs.len(), "Offering document versions");
        self.send_version_frames(peer, channel, docs, false, false);
    }

    /// Send one version frame per batch of documents.
    ///
    /// `force_partial` marks the frames as carrying less than everything we
    /// hold even when they fit in one, which is what a question about
    /// particular documents needs: without it the peer reads the names it
    /// does not see as documents we have never seen, and answers a question
    /// about one document with the whole space.
    fn send_version_frames(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        docs: BTreeMap<String, String>,
        reply: bool,
        force_partial: bool,
    ) {
        // A space with no documents still says so on the offering leg: that
        // is what tells a peer holding documents to send them to a replica
        // that has never seen any.
        if docs.is_empty() && reply {
            return;
        }
        let batches: Vec<BTreeMap<String, String>> = if docs.is_empty() {
            vec![BTreeMap::new()]
        } else {
            docs.into_iter()
                .collect::<Vec<_>>()
                .chunks(MAX_DOCS_PER_VERSION_FRAME)
                .map(|chunk| chunk.iter().cloned().collect())
                .collect()
        };
        // Every frame of a split offer is partial, including the last: the
        // inference that needs the complete list cannot be drawn from any
        // one of them, and there is nowhere to accumulate them that a
        // restart would not have to reconcile.
        let partial = force_partial || batches.len() > 1;
        for batch in batches {
            self.send_sync_frame(
                space,
                channel,
                &SyncBody::Versions {
                    reply,
                    partial,
                    docs: batch,
                },
            );
        }
    }

    /// Ask the peer for what we are missing in one document, without
    /// starting an exchange.
    ///
    /// Both flags are load-bearing. `reply` collects the catch-up while
    /// suppressing the counter-offer, so a request cannot start a chain of
    /// its own; `partial` keeps the peer from reading one name as the
    /// complete list and answering with every other document in the space.
    fn request_doc_catch_up(&mut self, space: &str, channel: &SyncChannel, doc: &str) {
        let version = match self.data_doc_version(space, doc) {
            Ok(version) => version,
            Err(err) => {
                warn!(space, doc, error = %err, "Could not read our version to ask for the gap");
                return;
            }
        };
        self.send_version_frames(
            space,
            channel,
            BTreeMap::from([(doc.to_string(), version)]),
            true,
            true,
        );
    }

    /// Seal one frame and put it on the ladder.
    ///
    /// The channel decides what "seal" means: a 1:1 space seals to its
    /// peer, a group space seals once under the group key. Everything
    /// above this function is written in terms of a space and a channel and
    /// does not know which of the two it is driving, which is what keeps
    /// one implementation of the exchange, the catch-up ladder and the
    /// import containment serving both.
    fn send_sync_frame(&mut self, space: &str, channel: &SyncChannel, body: &SyncBody) {
        let plaintext = match serde_json::to_string(body) {
            Ok(json) => format!(
                "{}{{\"v\":{},{}",
                internal_prefixes::DATA_V1,
                // The frame schema is the 1:1 version in both cases. The
                // group entry advertises that a peer *intercepts* these
                // frames inside a group ciphertext; it does not describe a
                // second frame shape, and minting one would mean two
                // parsers for identical bodies.
                DATA_SYNC_V1,
                // The body serializes as an object; splice the version in as
                // its first field rather than nesting, so a future version
                // can be read off a frame whose body shape it cannot parse.
                &json[1..]
            ),
            Err(err) => {
                warn!(space, error = %err, "Failed to encode sync frame");
                return;
            }
        };

        match channel {
            SyncChannel::Peer => self.send_sync_frame_to_peer(space, plaintext),
            SyncChannel::GroupBroadcast => {
                if let Err(err) = self.send_group_internal_frame(space, None, &plaintext) {
                    debug!(space, error = %err, "Group replication frame not sent");
                }
            }
            SyncChannel::GroupDirected(member) => {
                let member = member.clone();
                if let Err(err) = self.send_group_internal_frame(space, Some(&member), &plaintext) {
                    debug!(space, error = %err, "Directed replication frame not sent");
                }
            }
        }
    }

    /// The 1:1 leg: seal to the peer the space is named after.
    ///
    /// Deliberately the strict encryptor: a sync frame never initiates
    /// session establishment. Documents converge when the peers next talk,
    /// and provoking a handshake for an offer nobody asked for would make
    /// every reconnect noisier than the messaging it rides on.
    fn send_sync_frame_to_peer(&mut self, peer: &str, plaintext: String) {
        let encrypted = match self.encrypt_content_for_recipient_strict(peer, &plaintext) {
            Ok(content) => content,
            Err(err) => {
                // No session yet: nothing is lost. The version exchange runs
                // again on the next confirm, which is exactly the event that
                // would make this succeed.
                debug!(peer = %peer, error = %err, "Sync frame skipped: no session");
                return;
            }
        };

        let message = match self.create_message(peer, encrypted, None, None) {
            Ok(message) => message,
            Err(err) => {
                warn!(peer = %peer, error = %err, "Failed to build sync frame");
                return;
            }
        };

        if self.deduplicator.is_duplicate(&message.id) {
            return;
        }
        self.deduplicator.mark_seen(message.id.clone());

        // Tier 2: keep the plaintext so a resend after a re-key seals against
        // the peer's current epoch instead of replaying ciphertext they can
        // no longer open. The analogous session-confirm sender does not do
        // this and spends its whole retry budget on dead bytes; there is no
        // reason to inherit that. Staged after the duplicate check so the
        // entry belongs to a frame that is actually on the ladder.
        self.stage_outbox_reseal(
            &message.id,
            crate::protocol::types::OutboxReseal {
                content: plaintext,
                priority: message.priority,
                reply_to_msg: None,
                forwarded_from: None,
                content_type: message.content_type,
                media_metadata: None,
                rich: None,
            },
        );

        let previous_transport = self.transport_manager.current_transport();
        match self.transport_manager.send(&message) {
            Ok(()) => {
                let current = self.transport_manager.current_transport();
                let _ = self.handle_send_success(&message, current);
            }
            Err(err) => {
                let current = self.transport_manager.current_transport();
                let _ = self.handle_send_failure(&message, current.or(previous_transport));
                debug!(peer = %peer, error = %err, "Sync frame deferred to the outbox");
            }
        }
    }

    /// Push a freshly committed change to the peer this space replicates
    /// with.
    ///
    /// `origin` names the peer a change arrived from, when it did. Echoing a
    /// change back to whoever sent it is harmless — the merge absorbs it —
    /// but on a mesh it is a frame nobody needed, and a commit produced by
    /// applying a remote delta exports that delta again.
    pub(crate) fn push_data_delta(
        &mut self,
        space: &str,
        doc: &str,
        blob: &[u8],
        origin: Option<&str>,
    ) {
        let Some(channel) = self.space_channel(space, origin) else {
            return;
        };
        if blob.len() > MAX_SYNC_BLOB_BYTES {
            // Too big to inline, so the catch-up ladder has to fetch it: it
            // can answer with a compacted snapshot instead of raw history.
            // Offering now rather than waiting is what makes that happen. On
            // a link that never drops, "the peer's next version offer" is
            // not a time, and until it arrives both replicas believe they
            // agree.
            debug!(
                space,
                doc,
                bytes = blob.len(),
                "Change too large to push inline; offering versions instead"
            );
            self.nudge_data_sync(space, origin, "oversized_delta");
            return;
        }
        self.send_sync_frame(
            space,
            &channel,
            &SyncBody::Delta {
                doc: doc.to_string(),
                blob: BASE64.encode(blob),
            },
        );
    }

    // ---- inbound ----------------------------------------------------

    /// Handle a `__DATA_V1__` frame that arrived sealed from `sender`.
    ///
    /// Every outcome is terminal. A frame is never deferred for a
    /// data-layer reason: deferral means "this same ciphertext will succeed
    /// once the session is ready" and nothing else, so using it for a
    /// corrupt blob or a disabled layer would spend the sender's whole retry
    /// budget on a frame that cannot ever be accepted.
    pub(crate) fn handle_data_sync_frame(&mut self, sender: &str, body: &str) {
        // The space is the sender. Never a field on the frame: a peer that
        // could name the space could write into a document it replicates
        // with somebody else.
        self.handle_data_sync_frame_over(sender, &SyncChannel::Peer, sender, body);
    }

    /// A `__DATA_V1__` frame that arrived inside a *group* ciphertext.
    ///
    /// The space is the group, and the same structural argument holds as
    /// for 1:1: the group is not named by the frame, it is the group whose
    /// key opened the ciphertext. A member of one group cannot reach
    /// another group's documents without being a member of that group too,
    /// and the decrypt already proved membership and authenticated the
    /// sender.
    ///
    /// Answers go back to `sender` alone rather than to the roster. Every
    /// other member either has what was asked for or will ask for it
    /// themselves.
    #[cfg_attr(not(feature = "data"), allow(dead_code))]
    pub(crate) fn handle_group_data_sync_frame(
        &mut self,
        group_id: &str,
        sender: &str,
        body: &str,
    ) {
        self.handle_data_sync_frame_over(
            group_id,
            &SyncChannel::GroupDirected(sender.to_string()),
            sender,
            body,
        );
    }

    fn handle_data_sync_frame_over(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        sender: &str,
        body: &str,
    ) {
        if !self.config.data.enabled {
            // Parsing is never capability-gated, but applying is: the layer
            // is off, so the frame is dropped. Nothing is lost permanently —
            // whatever it carried is still on the sender, and the version
            // exchange after the layer comes back on asks for it again.
            debug!(peer = %sender, "Sync frame dropped: the data layer is off");
            return;
        }

        // The version is read before the body so a future frame shape is
        // declined on its version rather than failing to parse. Reversing
        // these two would make every future format look like corruption.
        let value: serde_json::Value = match serde_json::from_str(body) {
            Ok(value) => value,
            Err(err) => {
                warn!(peer = %sender, error = %err, "Malformed sync frame");
                return;
            }
        };
        match value.get("v").and_then(serde_json::Value::as_u64) {
            Some(v) if v == DATA_SYNC_V1 as u64 => {}
            other => {
                debug!(peer = %sender, version = ?other, "Sync frame from an unsupported version");
                return;
            }
        }
        let frame: SyncBody = match serde_json::from_value(value) {
            Ok(frame) => frame,
            Err(err) => {
                warn!(peer = %sender, error = %err, "Unreadable sync frame body");
                return;
            }
        };

        let space = space.to_string();
        let outcome = match frame {
            SyncBody::Versions {
                reply,
                partial,
                docs,
            } => self.answer_version_offer(&space, channel, reply, partial, docs),
            SyncBody::Delta { doc, blob } => {
                self.accept_remote_blob(&space, channel, &doc, &blob, BlobKind::Delta)
            }
            SyncBody::Snapshot { doc, blob } => {
                self.accept_remote_blob(&space, channel, &doc, &blob, BlobKind::Snapshot)
            }
            SyncBody::NeedSnapshot { doc } => self.answer_snapshot_request(&space, channel, &doc),
            SyncBody::NeedBlob { hash } => self.answer_blob_request(&space, channel, &hash),
            SyncBody::BlobGone { hash } => self.report_blob_gone(&space, &hash),
        };
        if let Err(err) = outcome {
            warn!(peer = %sender, error = %err, "Sync frame could not be handled");
        }
    }

    /// Answer a peer's version offer with what they are missing.
    fn answer_version_offer(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        reply: bool,
        partial: bool,
        theirs: BTreeMap<String, String>,
    ) -> Result<()> {
        let mut ours = self.data_sync_versions(space).unwrap_or_default();

        // Documents they have and we do not. A created document is empty,
        // and the only thing that ever fills it is this device asking for
        // its contents, so what is created here has to be asked about
        // before this frame is done with: the peer has already told us
        // everything it intends to and will not volunteer it again.
        let mut created = BTreeMap::new();
        for doc in theirs.keys() {
            if ours.contains_key(doc) || offline_protocol_data::validate_name(doc).is_err() {
                continue;
            }
            // The same door a blob for an unknown document goes through,
            // and the same ceiling. Counting the versions read above
            // instead would undercount by every document whose version
            // could not be read, which is how a space gets past its cap.
            if !self.data_space_admits_doc(space, doc, MAX_DOCS_PER_SPACE) {
                warn!(
                    space,
                    cap = MAX_DOCS_PER_SPACE,
                    "Space is at its document ceiling; ignoring the rest of the offer"
                );
                break;
            }
            if self.data_create_doc(space, doc).is_err() {
                continue;
            }
            // Folded into `ours` rather than re-read from the whole space
            // below: one version read per frame instead of two, and the
            // answer then describes exactly the documents this leg judged.
            match self.data_doc_version(space, doc) {
                Ok(version) => {
                    ours.insert(doc.clone(), version.clone());
                    created.insert(doc.clone(), version);
                }
                Err(err) => {
                    warn!(space, doc, error = %err, "Created a document from an offer but cannot read its version");
                }
            }
        }

        for (doc, ours_encoded) in &ours {
            let Some(theirs_encoded) = theirs.get(doc) else {
                // They did not name this document. On a complete offer that
                // means they have never seen it and the whole document is
                // the answer. On a partial one it means nothing: the name
                // may be in a frame that has not arrived, or in no frame at
                // all because the offer was about a single document.
                if !partial {
                    self.offer_catch_up(space, channel, doc, None);
                }
                continue;
            };
            if theirs_encoded == ours_encoded {
                continue;
            }
            match BASE64.decode(theirs_encoded) {
                Ok(token) => {
                    self.offer_catch_up(space, channel, doc, Some(VersionToken::from_bytes(token)))
                }
                Err(err) => {
                    warn!(space, doc, error = %err, "Undecodable version token in a sync frame");
                }
            }
        }

        // One answer, and the exchange is over. Answering an answer is how a
        // pair of replicas talk to each other forever.
        //
        // Either way the documents created above are asked about, which is
        // the invariant to keep: the counter-offer names them when there is
        // one, and when there is not they are asked for on their own. An
        // answering leg that skipped the question would leave a document
        // created from it empty for as long as the link stays up, because
        // the peer has said everything it had to say and nothing else here
        // ever asks.
        if !reply {
            self.send_version_frames(space, channel, ours, true, false);
        } else {
            self.send_version_frames(space, channel, created, true, true);
        }
        Ok(())
    }

    /// Send `doc` to the peer in whatever form can carry it.
    ///
    /// The ladder, cheapest first: the changes since their version, a whole
    /// document, and then nothing. "Nothing" is a real rung and it is
    /// reported rather than retried: a document too large for every inline
    /// form needs the media path, which the attachment stage brings, and a
    /// silent stall would be indistinguishable from a peer with no changes.
    fn offer_catch_up(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        doc: &str,
        theirs: Option<VersionToken>,
    ) {
        if let Some(token) = theirs {
            match self.data_catch_up(space, doc, &token) {
                Ok(CatchUp::UpToDate) => return,
                Ok(CatchUp::Updates(bytes)) if bytes.len() <= MAX_SYNC_BLOB_BYTES => {
                    self.send_sync_frame(
                        space,
                        channel,
                        &SyncBody::Delta {
                            doc: doc.to_string(),
                            blob: BASE64.encode(bytes),
                        },
                    );
                    return;
                }
                // Either too large to inline or a gap no run of changes can
                // express. Both are answered by the snapshot below.
                Ok(_) => {}
                Err(err) => {
                    warn!(space, doc, error = %err, "Could not compute catch-up");
                    return;
                }
            }
        }

        self.send_snapshot(space, channel, doc);
    }

    /// Send a whole document, the top rung and the answer to every refusal
    /// below it.
    ///
    /// Terminal by construction: a snapshot provokes no answer of its own,
    /// which is what lets the refusals underneath it ask for one freely.
    fn send_snapshot(&mut self, space: &str, channel: &SyncChannel, doc: &str) {
        match self.data_export_snapshot(space, doc) {
            Ok(bytes) if bytes.len() <= MAX_SYNC_BLOB_BYTES => {
                self.send_sync_frame(
                    space,
                    channel,
                    &SyncBody::Snapshot {
                        doc: doc.to_string(),
                        blob: BASE64.encode(bytes),
                    },
                );
            }
            // Too large for any frame. The rung above every frame is the
            // media path, which is where a document this size goes.
            Ok(bytes) => self.carry_snapshot_over_media(space, channel, doc, bytes),
            Err(err) => {
                warn!(space, doc, error = %err, "Could not export document for catch-up");
            }
        }
    }

    /// Send a document the media path carries, because no frame can.
    ///
    /// The top rung of the catch-up ladder and the last one. A document that
    /// cannot go this way either is reported rather than retried: nothing
    /// above this exists to try, and the two replicas stay apart until
    /// somebody makes the document smaller.
    ///
    /// Only ever 1:1 in this version. The media path is a transfer to a
    /// confirmed session and two members of a group need not have one with
    /// each other, so a group space that reaches this rung reports instead.
    fn carry_snapshot_over_media(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        doc: &str,
        bytes: Vec<u8>,
    ) {
        let size = bytes.len();
        let report = |protocol: &mut Self, reason: &str| {
            warn!(
                space,
                doc,
                bytes = size,
                reason,
                "Document cannot be replicated"
            );
            if let Ok(state) = crate::protocol::lock_shared_state(&protocol.shared_state) {
                state.emit_event(Event::DataDocUnsyncable {
                    space_id: space.to_string(),
                    doc_id: doc.to_string(),
                    bytes: size as u64,
                    reason: reason.to_string(),
                });
            }
        };

        if !matches!(channel, SyncChannel::Peer) {
            report(self, "group_space_has_no_media_path");
            return;
        }
        if size > MAX_MEDIA_SNAPSHOT_BYTES {
            report(self, "over_record_ceiling");
            return;
        }
        if !self.data_media_active(space) {
            report(self, "peer_cannot_carry_snapshots");
            return;
        }

        debug!(
            space,
            doc,
            bytes = size,
            "Carrying a snapshot over the media path"
        );
        if let Err(err) = self.send_media_inner(
            space.to_string(),
            bytes,
            doc.to_string(),
            ContentType::File,
            MediaSendOptions::default(),
            Some(DataPurpose::Snapshot {
                doc: doc.to_string(),
            }),
        ) {
            // Transient by nature: no confirmed session yet, the per-peer
            // transfer slots are full, or the protocol is not running.
            // Reported at debug and left for anti-entropy, which asks again
            // on the next exchange. Raising it as unsyncable would tell an
            // application a document is permanently stuck when it is merely
            // busy.
            debug!(space, doc, error = %err, "Snapshot carriage deferred");
        }
    }

    /// Answer a peer that cannot apply what we sent it.
    ///
    /// Only a document already held is served. A request naming an unknown
    /// one would otherwise create it, which is a way to spend this device's
    /// storage that does not even require a blob.
    fn answer_snapshot_request(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        doc: &str,
    ) -> Result<()> {
        if offline_protocol_data::validate_name(doc).is_err() {
            warn!(space, doc, "Snapshot request names an invalid document");
            return Ok(());
        }
        if !self.data_holds_doc(space, doc) {
            debug!(space, doc, "Snapshot request for a document we do not hold");
            return Ok(());
        }
        debug!(space, doc, "Serving a snapshot the peer asked for");
        self.send_snapshot(space, channel, doc);
        Ok(())
    }

    /// Apply a blob from a peer, with the containment the engine needs.
    fn accept_remote_blob(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        doc: &str,
        encoded: &str,
        kind: BlobKind,
    ) -> Result<()> {
        if offline_protocol_data::validate_name(doc).is_err() {
            warn!(space, doc, "Sync frame names an invalid document");
            return Ok(());
        }
        // A blob for a document this device does not hold creates one, so
        // this path is bounded by the same ceiling the offer loop is.
        if !self.data_space_admits_doc(space, doc, MAX_DOCS_PER_SPACE) {
            warn!(
                space,
                doc,
                cap = MAX_DOCS_PER_SPACE,
                "Space is at its document ceiling; refusing a new one"
            );
            return Ok(());
        }
        // Bound the decode by the frame budget: base64 grows by a third, so
        // checking the encoded length first refuses an oversized blob before
        // allocating for it.
        if encoded.len() > MAX_SYNC_BLOB_BYTES * 4 / 3 + 4 {
            warn!(space, doc, "Sync frame carries an oversized blob");
            return Ok(());
        }
        let blob = match BASE64.decode(encoded) {
            Ok(blob) => blob,
            Err(err) => {
                warn!(space, doc, error = %err, "Undecodable blob in a sync frame");
                return Ok(());
            }
        };
        if blob.len() > MAX_SYNC_BLOB_BYTES {
            warn!(
                space,
                doc,
                bytes = blob.len(),
                "Sync blob over the frame budget"
            );
            return Ok(());
        }

        self.apply_contained_blob(space, channel, doc, blob, kind)
    }

    /// Apply remote document bytes with the containment the engine needs.
    ///
    /// Split from [`Self::accept_remote_blob`] so bytes that arrived over
    /// the media path go through exactly this, rather than through a second
    /// copy of it. The road the bytes travelled says nothing about what they
    /// are: both roads carry an import that can still end the process, from
    /// a peer who is authenticated and may still be wrong.
    ///
    /// The frame budget is deliberately not checked here. It bounds what one
    /// frame may carry, which is a fact about frames; the media path has its
    /// own, larger bound because it has no such limit.
    fn apply_contained_blob(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        doc: &str,
        blob: Vec<u8>,
        kind: BlobKind,
    ) -> Result<()> {
        if offline_protocol_data::validate_name(doc).is_err() {
            warn!(space, doc, "A remote blob names an invalid document");
            return Ok(());
        }
        if !self.data_space_admits_doc(space, doc, MAX_DOCS_PER_SPACE) {
            warn!(
                space,
                doc,
                cap = MAX_DOCS_PER_SPACE,
                "Space is at its document ceiling; refusing a new one"
            );
            return Ok(());
        }

        let digest = blob_digest(&blob);
        let mut record = self.load_sync_record(space);
        if record.quarantined.contains(&digest) {
            // This blob was in flight when a previous run died. It may be a
            // coincidence and it may be the reason; either way applying it
            // again to find out is a bad trade, because the cost of being
            // wrong is the process and the cost of being right is one
            // document change that the sender still holds.
            warn!(space, doc, "Refusing a quarantined blob");
            return Ok(());
        }

        // Local edits waiting in memory are committed and pushed *before*
        // the import, so they leave on their own delta.
        //
        // A commit exports everything since the last one, regardless of who
        // authored it. So a local edit still pending when a remote blob
        // applies is folded into the same delta as the imported change, and
        // that delta is then suppressed as an echo toward the one peer it
        // was owed to. The edit is durable and announced to nobody: the next
        // commit exports only what came after it, and on a link that never
        // drops there is no reconnect to run an exchange that would notice.
        // Draining first costs one extra delta record and keeps the echo
        // suppression judging a delta that really is nothing but the echo.
        //
        // Reported and stepped over rather than propagated. This is our own
        // housekeeping, and a document held over its cap fails here on every
        // flush while still needing to accept the remote deletions that
        // bring it back under: refusing the import for it would close the
        // only door out.
        //
        // Ahead of the marker below rather than inside its window, so the
        // window stays the length of one import. A crash during this flush
        // would otherwise quarantine a blob the engine never saw.
        //
        // The two errors say opposite things about what is still pending, so
        // they are told apart. `DocTooLarge` is raised after the delta record
        // was written, pushed and announced, and only the size verdict that
        // follows it failed: nothing is left in the pending set and there is
        // nothing to compensate for. Any other error may have left the commit
        // un-exported, because the delta-write failure rewinds it back into
        // the pending set. The import's own flush would then fold that edit
        // into the imported change and suppress the pair toward the one peer
        // it was owed to, which is exactly the loss this pre-flush exists to
        // prevent.
        let mut preflush_left_edits_pending = false;
        match self.data_flush(space, doc) {
            Ok(()) => {}
            Err(Error::DocTooLarge { .. }) => {
                debug!(
                    space,
                    doc,
                    "Local edits were flushed and pushed before applying a remote change; the \
                     document is over its cap and refuses further growth"
                );
            }
            Err(err) => {
                warn!(
                    space,
                    doc,
                    error = %err,
                    "Could not flush local edits before applying a remote change; they may \
                     still be pending"
                );
                preflush_left_edits_pending = true;
            }
        }

        // Written *before* the engine is called. That ordering is the whole
        // mechanism: a blob that ends the process leaves its digest behind,
        // so the sender's next retry is refused instead of ending the next
        // process too.
        record.in_flight = Some(digest.clone());
        self.persist_sync_record(space, &record);

        let outcome = self.data_apply_remote(space, doc, &blob);

        record.in_flight = None;
        self.persist_sync_record(space, &record);

        // Read before the match below consumes the outcome.
        let applied = matches!(outcome, Ok(RemoteImport::Applied));

        match outcome {
            Ok(RemoteImport::Applied) => {
                debug!(space, doc, "Applied a remote change");
            }
            Ok(RemoteImport::AlreadyHave) => {}
            Ok(RemoteImport::Parked) => match kind {
                // The change is held behind a predecessor that has not
                // arrived. Asking for the gap now is what makes the parked
                // change durable: it lives only in the engine's memory, so a
                // restart before the predecessor lands would lose it, and
                // the version exchange is what brings both back. Scoped to
                // this document, because this document is what is missing
                // something.
                BlobKind::Delta => {
                    debug!(space, doc, "Remote change parked; asking for the gap");
                    self.request_doc_catch_up(space, channel, doc);
                }
                // A snapshot is the top of the ladder. Asking again returns
                // the same bytes, so this is reported and left, rather than
                // retried until one side gives up or the battery does.
                BlobKind::Snapshot => {
                    warn!(
                        space,
                        doc,
                        "A snapshot from the peer did not apply; these replicas cannot be \
                         reconciled inline"
                    );
                }
            },
            Ok(RemoteImport::RefusedTrimmedHistory) => match kind {
                // Nothing the peer can compute from our version closes this
                // gap: they would recompute the same refused delta, and
                // answering with a version offer is how that becomes an
                // exchange neither side can end. Name what would work.
                BlobKind::Delta => {
                    debug!(space, doc, "Remote change needs a snapshot; asking for one");
                    self.send_sync_frame(
                        space,
                        channel,
                        &SyncBody::NeedSnapshot {
                            doc: doc.to_string(),
                        },
                    );
                }
                // The end of the line, and the one outcome this layer
                // cannot recover from. The peer forked below a point this
                // replica compacted away, so the ancestors their changes
                // need were deleted here rather than left out of the
                // message: no frame carries them back, including the whole
                // document, which is what just arrived. Reported plainly
                // rather than retried, because a retry returns these same
                // bytes and refusing them is the only thing keeping the
                // process alive.
                BlobKind::Snapshot => {
                    warn!(
                        space,
                        doc,
                        "A snapshot from the peer cannot merge into this replica; the two \
                         have diverged past what replication can reconcile"
                    );
                }
            },
            Err(err) => match kind {
                BlobKind::Delta => {
                    warn!(space, doc, error = %err, "Remote change refused");
                }
                // The top of the ladder, so there is nothing above it to
                // ask for. Unreconcilable divergence is refused before it
                // gets here rather than failing here, which leaves this for
                // bytes that are simply bad.
                BlobKind::Snapshot => {
                    warn!(space, doc, error = %err, "A snapshot from the peer was unreadable");
                }
            },
        }

        // A pre-flush that failed followed by an import that applied is the
        // one combination where a local edit can have been folded into the
        // imported change and then suppressed toward the peer it was owed
        // to. Nothing on the wire carries it and no trigger is left to
        // notice, so the gap is announced and the peer asks for what it is
        // missing. The alternative, sending the fold on with the suppression
        // lifted, would hand the peer its own change back.
        //
        // Gated on `Applied` for the reason that also makes it terminate.
        // The fold needs the import's own flush to have succeeded, and a
        // storage failure that is still failing fails that one too, which
        // makes the import answer `Err`. So every nudge costs a fresh
        // transient failure that recovered inside one frame, and the offers
        // cannot sustain each other. `Applied` and not "anything but `Err`"
        // for the same reason: the outcomes that did not apply left the edit
        // pending, where the next pre-flush still owes it to the peer.
        //
        // This is why a document over its cap must answer `Applied` rather
        // than `DocTooLarge`, and does: its flush wrote the fold and then
        // refused further growth, so the edit is stranded exactly as here,
        // on the documents whose only pending edit is the deletion that
        // brings them back under.
        //
        // One residual is left uncompensated on purpose. A `persist_space`
        // failure after a flush that succeeded also strands a fold, and is
        // indistinguishable here from a flush that failed. Offering on every
        // `Err` would cover it and break the argument above: a store that
        // keeps failing would then emit an offer per frame, and those do
        // sustain each other. The narrower gap is the better trade.
        //
        // `origin` is `None`, not the space: what may be stranded here is
        // this device's own edit, so naming the space would suppress the
        // announcement toward the only peer that could act on it.
        if preflush_left_edits_pending && applied {
            self.nudge_data_sync(space, None, "preflush_failure");
        }

        Ok(())
    }

    // ---- attachments -------------------------------------------------

    /// Key of a pending fetch: the space asked in, then the blob wanted.
    ///
    /// Composed rather than keyed by hash alone because the same bytes may
    /// be referenced from two spaces, and an answer from one peer must not
    /// close a question put to another.
    fn attachment_fetch_key(space: &str, hash: &str) -> String {
        format!("{space}{GROUP_OFFER_KEY_SEP}{hash}")
    }

    /// The SHA-256 of `bytes`, in the spelling an attachment reference uses.
    ///
    /// Exposed because an application writing a reference needs the address
    /// of its own bytes, and computing it anywhere else risks a second
    /// spelling of the same hash.
    pub fn data_attachment_hash(bytes: &[u8]) -> String {
        hex_encode(&Sha256::digest(bytes))
    }

    /// Ask the peer a 1:1 space is named after for the bytes behind a
    /// reference.
    ///
    /// Pull rather than push, and the reason is bandwidth somebody else
    /// pays for: a space may hold references to more bytes than a phone
    /// wants over Bluetooth, so the decision to spend that belongs to the
    /// application, taken per blob, at the moment somebody opens one.
    ///
    /// The answer arrives later as [`Event::DataAttachmentReceived`] or
    /// [`Event::DataAttachmentUnavailable`], never inline.
    ///
    /// Group spaces are refused in this version. A blob travels the media
    /// path, which is a 1:1 transfer to a confirmed session, and two members
    /// of a group need not have one with each other.
    pub fn data_fetch_attachment(&mut self, space: &str, hash: &str) -> Result<()> {
        if !self.config.data.enabled {
            return Err(Error::InvalidArgument(
                "the data layer is disabled".to_string(),
            ));
        }
        Self::validate_attachment_hash(hash)?;
        if self.group_space_roster(space).is_some() {
            return Err(Error::InvalidArgument(format!(
                "space {space} is a group; attachment carriage is 1:1 in this version"
            )));
        }
        if !self.data_media_active(space) {
            return Err(Error::InvalidArgument(format!(
                "peer {space} did not advertise attachment carriage"
            )));
        }

        let key = Self::attachment_fetch_key(space, hash);
        self.expire_attachment_fetches();
        if self.pending_attachment_fetches.len() >= MAX_PENDING_ATTACHMENT_FETCHES
            && !self.pending_attachment_fetches.contains_key(&key)
        {
            if let Some(oldest) = self
                .pending_attachment_fetches
                .iter()
                .min_by_key(|(_, asked_at)| **asked_at)
                .map(|(key, _)| key.clone())
            {
                self.pending_attachment_fetches.remove(&oldest);
            }
        }
        self.pending_attachment_fetches.insert(key, Instant::now());

        self.send_sync_frame(
            space,
            &SyncChannel::Peer,
            &SyncBody::NeedBlob {
                hash: hash.to_string(),
            },
        );
        Ok(())
    }

    /// Answer a peer's request with the bytes.
    ///
    /// The bytes come from the application because this SDK never held them:
    /// a blob does not enter a document and does not enter protocol state,
    /// so the only copy is wherever the app put it.
    ///
    /// Refuses bytes that do not hash to `hash`. That check is here rather
    /// than only on the receiving side so a mistake is reported to the app
    /// that made it, while it still has the file in hand, instead of
    /// travelling the whole media path to be refused by somebody else.
    pub fn data_provide_attachment(
        &mut self,
        space: &str,
        peer: &str,
        hash: &str,
        bytes: Vec<u8>,
    ) -> Result<()> {
        if !self.config.data.enabled {
            return Err(Error::InvalidArgument(
                "the data layer is disabled".to_string(),
            ));
        }
        Self::validate_attachment_hash(hash)?;
        if bytes.is_empty() {
            return Err(Error::InvalidArgument(
                "an attachment must have bytes".to_string(),
            ));
        }
        if space != peer {
            return Err(Error::InvalidArgument(format!(
                "space {space} is not the 1:1 space with {peer}"
            )));
        }
        let actual = Self::data_attachment_hash(&bytes);
        if actual != hash {
            return Err(Error::InvalidArgument(format!(
                "these bytes hash to {actual}, not to {hash}"
            )));
        }

        self.send_media_inner(
            peer.to_string(),
            bytes,
            // The media layer wants a file name and an attachment has none
            // that is load-bearing: the display name lives on the reference
            // in the document, where every replica can already read it.
            hash.to_string(),
            ContentType::File,
            MediaSendOptions::default(),
            Some(DataPurpose::Attachment {
                hash: hash.to_string(),
            }),
        )?;
        Ok(())
    }

    /// Tell a peer their fetch will not be answered.
    ///
    /// A first-class answer rather than silence. The asking side cannot
    /// otherwise tell a peer that no longer holds the bytes from one that is
    /// merely slow, and shows a person a spinner that never resolves.
    pub fn data_decline_attachment(&mut self, space: &str, peer: &str, hash: &str) -> Result<()> {
        Self::validate_attachment_hash(hash)?;
        if space != peer {
            return Err(Error::InvalidArgument(format!(
                "space {space} is not the 1:1 space with {peer}"
            )));
        }
        self.send_sync_frame(
            space,
            &SyncChannel::Peer,
            &SyncBody::BlobGone {
                hash: hash.to_string(),
            },
        );
        Ok(())
    }

    fn validate_attachment_hash(hash: &str) -> Result<()> {
        offline_protocol_data::validate_attachment(hash, 1, None, None)
            .map_err(|err| Error::InvalidArgument(err.to_string()))
    }

    /// Drop fetches nobody answered, so the bound counts live questions.
    fn expire_attachment_fetches(&mut self) {
        let now = Instant::now();
        self.pending_attachment_fetches
            .retain(|_, asked_at| now.duration_since(*asked_at) < ATTACHMENT_FETCH_TIMEOUT);
    }

    /// A peer wants the bytes behind a reference.
    ///
    /// Handed straight to the application, because the application is the
    /// only party that has them. Rate limited per blob on the window that
    /// suppresses version offers: a peer retrying a fetch must not turn into
    /// a stream of events, and the answer to the second ask within the
    /// window is the same as the answer to the first.
    fn answer_blob_request(
        &mut self,
        space: &str,
        channel: &SyncChannel,
        hash: &str,
    ) -> Result<()> {
        if !matches!(channel, SyncChannel::Peer) {
            warn!(
                space,
                "A blob request arrived inside a group; attachment carriage is 1:1"
            );
            return Ok(());
        }
        if Self::validate_attachment_hash(hash).is_err() {
            warn!(space, "Blob request names something that is not a hash");
            return Ok(());
        }
        let window = format!("{space}{GROUP_OFFER_KEY_SEP}blob{GROUP_OFFER_KEY_SEP}{hash}");
        if !self.data_sync_offer_due(&window) {
            debug!(space, "Blob request repeated inside its window; ignoring");
            return Ok(());
        }
        if let Ok(state) = crate::protocol::lock_shared_state(&self.shared_state) {
            state.emit_event(Event::DataAttachmentRequested {
                space_id: space.to_string(),
                peer_id: space.to_string(),
                hash: hash.to_string(),
            });
        }
        Ok(())
    }

    /// A peer says the bytes are gone. End the fetch.
    fn report_blob_gone(&mut self, space: &str, hash: &str) -> Result<()> {
        let key = Self::attachment_fetch_key(space, hash);
        // Only a fetch this device actually made is reported. Otherwise a
        // peer could emit refusals for blobs nobody asked about.
        if self.pending_attachment_fetches.remove(&key).is_none() {
            debug!(space, "A blob refusal answers no question we asked");
            return Ok(());
        }
        if let Ok(state) = crate::protocol::lock_shared_state(&self.shared_state) {
            state.emit_event(Event::DataAttachmentUnavailable {
                space_id: space.to_string(),
                peer_id: space.to_string(),
                hash: hash.to_string(),
                reason: "declined".to_string(),
            });
        }
        Ok(())
    }

    // ---- what arrives over the media path ------------------------------

    /// A data-purposed media transfer completed. Route it by its purpose.
    ///
    /// The space is the authenticated wire sender, never anything the
    /// transfer said about itself, which is the same rule every sync frame
    /// follows and for the same reason: a space a peer can name is a space a
    /// peer can reach.
    pub(crate) fn accept_data_media_payload(
        &mut self,
        sender: &str,
        purpose: &DataPurpose,
        bytes: Vec<u8>,
    ) {
        match purpose {
            DataPurpose::Attachment { hash } => {
                let key = Self::attachment_fetch_key(sender, hash);
                if self.pending_attachment_fetches.remove(&key).is_none() {
                    warn!(
                        peer = %sender,
                        "Attachment bytes arrived for a fetch this device never made; dropping"
                    );
                    return;
                }
                // The address is checked against the bytes, not against what
                // the sender says about them. This is what makes fetching
                // from an authenticated peer safe without trusting that
                // peer: the worst a wrong answer achieves is no answer.
                let actual = Self::data_attachment_hash(&bytes);
                if &actual != hash {
                    warn!(
                        peer = %sender,
                        "Attachment bytes do not hash to what was asked for; dropping"
                    );
                    if let Ok(state) = crate::protocol::lock_shared_state(&self.shared_state) {
                        state.emit_event(Event::DataAttachmentUnavailable {
                            space_id: sender.to_string(),
                            peer_id: sender.to_string(),
                            hash: hash.clone(),
                            reason: "hash_mismatch".to_string(),
                        });
                    }
                    return;
                }
                if let Ok(state) = crate::protocol::lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::DataAttachmentReceived {
                        space_id: sender.to_string(),
                        peer_id: sender.to_string(),
                        hash: hash.clone(),
                        data: BASE64.encode(&bytes),
                    });
                }
            }
            DataPurpose::Snapshot { doc } => {
                if bytes.len() > MAX_MEDIA_SNAPSHOT_BYTES {
                    warn!(
                        peer = %sender,
                        bytes = bytes.len(),
                        "A carried snapshot is larger than a record can hold; refusing"
                    );
                    return;
                }
                // Straight into the containment every remote blob goes
                // through. Arriving by a longer road changes nothing about
                // what these bytes are: an import that can still end the
                // process, from a peer who is exactly who they say they are.
                let doc = doc.clone();
                if let Err(err) = self.apply_contained_blob(
                    sender,
                    &SyncChannel::Peer,
                    &doc,
                    bytes,
                    BlobKind::Snapshot,
                ) {
                    warn!(peer = %sender, doc, error = %err, "A carried snapshot was refused");
                }
            }
        }
    }

    /// A data-purposed media transfer failed. Say so where it can be acted on.
    pub(crate) fn report_data_media_failure(
        &mut self,
        sender: &str,
        purpose: &DataPurpose,
        reason: &str,
    ) {
        match purpose {
            DataPurpose::Attachment { hash } => {
                let key = Self::attachment_fetch_key(sender, hash);
                self.pending_attachment_fetches.remove(&key);
                if let Ok(state) = crate::protocol::lock_shared_state(&self.shared_state) {
                    state.emit_event(Event::DataAttachmentUnavailable {
                        space_id: sender.to_string(),
                        peer_id: sender.to_string(),
                        hash: hash.clone(),
                        reason: reason.to_string(),
                    });
                }
            }
            // Nothing to report to an application: a snapshot that did not
            // arrive is a gap anti-entropy will find again, and the next
            // exchange asks for it afresh.
            DataPurpose::Snapshot { doc } => {
                debug!(peer = %sender, doc, reason, "A carried snapshot did not arrive");
            }
        }
    }

    // ---- the crash record -------------------------------------------

    /// Read a space's replication bookkeeping, promoting anything left in
    /// flight by a previous run into the quarantine.
    fn load_sync_record(&mut self, space: &str) -> SyncRecord {
        let Some(storage) = self.data_storage_for_sync() else {
            return SyncRecord::default();
        };
        let mut record =
            match self.read_state_record(storage.as_ref(), storage_keys::DATA_SYNC, space) {
                Ok(Some(bytes)) => serde_json::from_slice::<SyncRecord>(&bytes).unwrap_or_default(),
                _ => SyncRecord::default(),
            };
        if let Some(digest) = record.in_flight.take() {
            warn!(
                space,
                "A previous run did not survive applying a remote change; quarantining it"
            );
            if !record.quarantined.contains(&digest) {
                record.quarantined.push(digest);
                while record.quarantined.len() > MAX_QUARANTINED_BLOBS {
                    record.quarantined.remove(0);
                }
            }
            self.persist_sync_record(space, &record);
        }
        record
    }

    fn persist_sync_record(&mut self, space: &str, record: &SyncRecord) {
        let Some(storage) = self.data_storage_for_sync() else {
            return;
        };
        let bytes = match serde_json::to_vec(record) {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(space, error = %err, "Could not encode replication bookkeeping");
                return;
            }
        };
        if let Err(err) =
            self.write_state_record(storage.as_ref(), storage_keys::DATA_SYNC, space, &bytes)
        {
            // Not fatal, and deliberately not a refusal to apply: failing to
            // write the marker costs the crash protection for this one blob,
            // while refusing would stop replication on a storage hiccup.
            warn!(space, error = %err, "Could not persist replication bookkeeping");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_carries_its_version_where_a_future_build_can_find_it() {
        // The version has to be readable off a frame whose body this build
        // cannot parse, or every future format looks like corruption to
        // every older install.
        let body = SyncBody::Delta {
            doc: "notes".to_string(),
            blob: "AAAA".to_string(),
        };
        let json = serde_json::to_string(&body).unwrap();
        let framed = format!("{{\"v\":{},{}", DATA_SYNC_V1, &json[1..]);

        let value: serde_json::Value = serde_json::from_str(&framed).unwrap();
        assert_eq!(value.get("v").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(value.get("k").and_then(|v| v.as_str()), Some("delta"));

        let round_trip: SyncBody = serde_json::from_value(value).unwrap();
        assert!(matches!(round_trip, SyncBody::Delta { doc, .. } if doc == "notes"));
    }

    #[test]
    fn a_version_frame_round_trips() {
        let body = SyncBody::Versions {
            reply: true,
            partial: true,
            docs: BTreeMap::from([("notes".to_string(), "dgA=".to_string())]),
        };
        let json = serde_json::to_string(&body).unwrap();
        let parsed: SyncBody = serde_json::from_str(&json).unwrap();
        match parsed {
            SyncBody::Versions {
                reply,
                partial,
                docs,
            } => {
                assert!(reply);
                assert!(partial);
                assert_eq!(docs.get("notes").map(String::as_str), Some("dgA="));
            }
            other => panic!("expected a version frame, got {other:?}"),
        }
    }

    #[test]
    fn an_offer_that_says_nothing_about_completeness_is_read_as_complete() {
        // The two flags default to the shape an ordinary whole-space offer
        // has, so a frame that omits them behaves like one. Getting this
        // backwards would silence the inference that carries a document a
        // peer has never seen.
        let parsed: SyncBody = serde_json::from_str(r#"{"k":"vv","docs":{}}"#).unwrap();
        match parsed {
            SyncBody::Versions {
                reply,
                partial,
                docs,
            } => {
                assert!(!reply);
                assert!(!partial);
                assert!(docs.is_empty());
            }
            other => panic!("expected a version frame, got {other:?}"),
        }
    }

    #[test]
    fn a_snapshot_request_round_trips_and_carries_no_bytes() {
        // It names a document and nothing else: the request exists because
        // the asker cannot express its own gap, so anything it added would
        // be the thing that was already refused.
        let body = SyncBody::NeedSnapshot {
            doc: "notes".to_string(),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(json, r#"{"k":"need_snap","doc":"notes"}"#);

        let parsed: SyncBody = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SyncBody::NeedSnapshot { doc } if doc == "notes"));
    }

    #[test]
    fn digests_separate_blobs_and_repeat_for_the_same_one() {
        assert_eq!(blob_digest(b"one"), blob_digest(b"one"));
        assert_ne!(blob_digest(b"one"), blob_digest(b"two"));
        assert_eq!(blob_digest(b"one").len(), 32);
    }
}
