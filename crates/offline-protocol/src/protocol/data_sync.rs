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

use crate::error::Result;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::types::{storage_keys, DATA_SYNC_V1};
use crate::protocol::OfflineProtocol;

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

/// Blob digests remembered per space after a crash.
///
/// Small on purpose. The list only has to outlive the sender's retries of
/// one blob, and every entry in it is a document change we have decided
/// never to apply, so a long list is a liability rather than a safety net.
const MAX_QUARANTINED_BLOBS: usize = 32;

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
fn blob_digest(blob: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(blob);
    // Half a SHA-256 is 128 bits, which is far past what a collision would
    // need to be worth engineering: the prize is having one specific blob
    // refused on one device.
    hex_encode(&hasher.finalize()[..16])
}

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
        let now = Instant::now();
        if let Some(last) = self.last_data_sync_offer.get(peer) {
            if now.duration_since(*last) < DATA_SYNC_OFFER_INTERVAL {
                return;
            }
        }
        self.last_data_sync_offer.insert(peer.to_string(), now);

        let docs = match self.data_sync_versions(peer) {
            Ok(docs) => docs,
            Err(err) => {
                debug!(peer = %peer, cause, error = %err, "Could not read document versions");
                return;
            }
        };
        debug!(peer = %peer, cause, docs = docs.len(), "Offering document versions");
        self.send_version_frames(peer, docs, false);
    }

    /// Send one version frame per batch of documents.
    fn send_version_frames(&mut self, peer: &str, docs: BTreeMap<String, String>, reply: bool) {
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
        for batch in batches {
            self.send_sync_frame(peer, &SyncBody::Versions { reply, docs: batch });
        }
    }

    /// Seal one frame and put it on the ladder.
    ///
    /// Deliberately the strict encryptor: a sync frame never initiates
    /// session establishment. Documents converge when the peers next talk,
    /// and provoking a handshake for an offer nobody asked for would make
    /// every reconnect noisier than the messaging it rides on.
    fn send_sync_frame(&mut self, peer: &str, body: &SyncBody) {
        let plaintext = match serde_json::to_string(body) {
            Ok(json) => format!(
                "{}{{\"v\":{},{}",
                internal_prefixes::DATA_V1,
                DATA_SYNC_V1,
                // The body serializes as an object; splice the version in as
                // its first field rather than nesting, so a future version
                // can be read off a frame whose body shape it cannot parse.
                &json[1..]
            ),
            Err(err) => {
                warn!(peer = %peer, error = %err, "Failed to encode sync frame");
                return;
            }
        };

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

        // Tier 2: keep the plaintext so a resend after a re-key seals against
        // the peer's current epoch instead of replaying ciphertext they can
        // no longer open. The analogous session-confirm sender does not do
        // this and spends its whole retry budget on dead bytes; there is no
        // reason to inherit that.
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

        if self.deduplicator.is_duplicate(&message.id) {
            return;
        }
        self.deduplicator.mark_seen(message.id.clone());

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
        if origin == Some(space) || !self.data_sync_active(space) {
            return;
        }
        if blob.len() > MAX_SYNC_BLOB_BYTES {
            // Too big to inline. The peer's next version offer sees the gap
            // and asks for it through the catch-up ladder, which can answer
            // with a compacted snapshot instead of raw history.
            debug!(
                space,
                doc,
                bytes = blob.len(),
                "Change too large to push inline; leaving it to the version exchange"
            );
            return;
        }
        let peer = space.to_string();
        self.send_sync_frame(
            &peer,
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

        // The space is the sender. Never a field on the frame: a peer that
        // could name the space could write into a document it replicates
        // with somebody else.
        let space = sender.to_string();
        let outcome = match frame {
            SyncBody::Versions { reply, docs } => self.answer_version_offer(&space, reply, docs),
            SyncBody::Delta { doc, blob } | SyncBody::Snapshot { doc, blob } => {
                self.accept_remote_blob(&space, &doc, &blob)
            }
        };
        if let Err(err) = outcome {
            warn!(peer = %sender, error = %err, "Sync frame could not be handled");
        }
    }

    /// Answer a peer's version offer with what they are missing.
    fn answer_version_offer(
        &mut self,
        space: &str,
        reply: bool,
        theirs: BTreeMap<String, String>,
    ) -> Result<()> {
        let ours = self.data_sync_versions(space).unwrap_or_default();

        // Documents they have and we do not: created here so the next leg
        // offers a version for them and they send the content.
        for doc in theirs.keys() {
            if !ours.contains_key(doc) && offline_protocol_data::validate_name(doc).is_ok() {
                let _ = self.data_create_doc(space, doc);
            }
        }

        for (doc, ours_encoded) in &ours {
            let Some(theirs_encoded) = theirs.get(doc) else {
                // They have never seen this document at all.
                self.offer_catch_up(space, doc, None);
                continue;
            };
            if theirs_encoded == ours_encoded {
                continue;
            }
            match BASE64.decode(theirs_encoded) {
                Ok(token) => self.offer_catch_up(space, doc, Some(VersionToken::from_bytes(token))),
                Err(err) => {
                    warn!(space, doc, error = %err, "Undecodable version token in a sync frame");
                }
            }
        }

        // One answer, and the exchange is over. Answering an answer is how a
        // pair of replicas talk to each other forever.
        if !reply {
            let ours = self.data_sync_versions(space).unwrap_or_default();
            let peer = space.to_string();
            self.send_version_frames(&peer, ours, true);
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
    fn offer_catch_up(&mut self, space: &str, doc: &str, theirs: Option<VersionToken>) {
        if let Some(token) = theirs {
            match self.data_catch_up(space, doc, &token) {
                Ok(CatchUp::UpToDate) => return,
                Ok(CatchUp::Updates(bytes)) if bytes.len() <= MAX_SYNC_BLOB_BYTES => {
                    let peer = space.to_string();
                    self.send_sync_frame(
                        &peer,
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

        match self.data_export_snapshot(space, doc) {
            Ok(bytes) if bytes.len() <= MAX_SYNC_BLOB_BYTES => {
                let peer = space.to_string();
                self.send_sync_frame(
                    &peer,
                    &SyncBody::Snapshot {
                        doc: doc.to_string(),
                        blob: BASE64.encode(bytes),
                    },
                );
            }
            Ok(bytes) => {
                warn!(
                    space,
                    doc,
                    bytes = bytes.len(),
                    "Document is too large to replicate in one frame; it needs the media path"
                );
            }
            Err(err) => {
                warn!(space, doc, error = %err, "Could not export document for catch-up");
            }
        }
    }

    /// Apply a blob from a peer, with the containment the engine needs.
    fn accept_remote_blob(&mut self, space: &str, doc: &str, encoded: &str) -> Result<()> {
        if offline_protocol_data::validate_name(doc).is_err() {
            warn!(space, doc, "Sync frame names an invalid document");
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

        // Written *before* the engine is called. That ordering is the whole
        // mechanism: a blob that ends the process leaves its digest behind,
        // so the sender's next retry is refused instead of ending the next
        // process too.
        record.in_flight = Some(digest.clone());
        self.persist_sync_record(space, &record);

        let outcome = self.data_apply_remote(space, doc, &blob);

        record.in_flight = None;
        self.persist_sync_record(space, &record);

        match outcome {
            Ok(RemoteImport::Applied) => {
                debug!(space, doc, "Applied a remote change");
            }
            Ok(RemoteImport::AlreadyHave) => {}
            Ok(RemoteImport::Parked) => {
                // The change is held behind a predecessor that has not
                // arrived. Asking for the gap now is what makes the parked
                // change durable: it lives only in the engine's memory, so a
                // restart before the predecessor lands would lose it, and
                // the version exchange is what brings both back.
                debug!(space, doc, "Remote change parked; asking for the gap");
                let peer = space.to_string();
                let ours = self.data_sync_versions(&peer).unwrap_or_default();
                self.send_version_frames(&peer, ours, false);
            }
            Ok(RemoteImport::RefusedTrimmedHistory) => {
                debug!(space, doc, "Remote change needs a snapshot; asking for one");
                let peer = space.to_string();
                let ours = self.data_sync_versions(&peer).unwrap_or_default();
                self.send_version_frames(&peer, ours, false);
            }
            Err(err) => {
                warn!(space, doc, error = %err, "Remote change refused");
            }
        }
        Ok(())
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
            docs: BTreeMap::from([("notes".to_string(), "dgA=".to_string())]),
        };
        let json = serde_json::to_string(&body).unwrap();
        let parsed: SyncBody = serde_json::from_str(&json).unwrap();
        match parsed {
            SyncBody::Versions { reply, docs } => {
                assert!(reply);
                assert_eq!(docs.get("notes").map(String::as_str), Some("dgA="));
            }
            other => panic!("expected a version frame, got {other:?}"),
        }
    }

    #[test]
    fn digests_separate_blobs_and_repeat_for_the_same_one() {
        assert_eq!(blob_digest(b"one"), blob_digest(b"one"));
        assert_ne!(blob_digest(b"one"), blob_digest(b"two"));
        assert_eq!(blob_digest(b"one").len(), 32);
    }
}
