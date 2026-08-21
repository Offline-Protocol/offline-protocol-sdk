//! Minting outbound frames, and refusing inbound ones that cannot prove who
//! sent them.
//!
//! # The gate
//!
//! Every control frame in this protocol carries an Ed25519 signature in two
//! metadata keys, over a domain-separated canonical payload built from the
//! sender, the id, the recipient and the content. A leaf verifies three things
//! and refuses on any of them:
//!
//! 1. the signature metadata is present and complete,
//! 2. the signature verifies under the key the frame presents,
//! 3. **the presented key derives to the address the frame claims to be from**.
//!
//! The third is the one that means anything. The first two prove a key signed
//! this; only the third proves it is the peer's key. Both halves of a
//! mismatch, and an identifier that is not an address at all, are the same
//! refusal: an identifier with no derivation to check is not a claim that
//! needs waving through, it is the bypass, and answering "acceptable" for it
//! is how an attacker skips the gate by claiming a nickname.

use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use offline_protocol_core::{
    Address, AppId, LamportClock, Message, MessageId, MessagePriority, Timestamp, UserId,
};
use offline_protocol_sealed::{
    control_signing_payload, derive_address, CTRL_PK_META_KEY, CTRL_SIG_META_KEY,
};

use crate::error::{LeafError, Result};
use crate::identity::{self, Identity};
use crate::store::{LeafStore, KEY_TYPE_IDENTITY};

const KEY_ID_COUNTER: &str = "send_counter";

/// Reads, advances and persists the device's send counter.
///
/// One counter serves both jobs a phone uses entropy and a clock for: it makes
/// a unique [`MessageId`] without drawing randomness, and it is the Lamport
/// value that orders this device's sends. Ids here are compared for equality
/// by a receiver's deduplicator and are never assumed unpredictable, so a
/// counter is the right primitive rather than a weaker substitute for one.
///
/// It is persisted **before** the frame it numbers exists, so a power cut
/// costs a skipped id rather than a repeated one. A repeated id would be
/// silently swallowed by the peer's deduplicator, which is a message that
/// vanishes with nothing anywhere reporting it.
fn next_counter(store: &Arc<dyn LeafStore>) -> Result<u64> {
    let current = store
        .load(KEY_TYPE_IDENTITY, KEY_ID_COUNTER)
        .map_err(|e| LeafError::Storage(e.to_string()))?
        .and_then(|raw| <[u8; 8]>::try_from(raw.as_slice()).ok())
        .map(u64::from_be_bytes)
        .unwrap_or(0);

    let next = current.saturating_add(1);
    store
        .store(KEY_TYPE_IDENTITY, KEY_ID_COUNTER, &next.to_be_bytes())
        .map_err(|e| LeafError::Storage(e.to_string()))?;
    Ok(next)
}

/// Builds a message id that no other device mints.
///
/// The first eight bytes come from the device's own address, which is a hash
/// of its identity key, and the last eight are the send counter. Two devices
/// collide only if their addresses collide, which is the same margin every
/// other identity claim in this protocol rests on.
fn message_id(address: &Address, counter: u64) -> MessageId {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&address.hash_bytes()[..8]);
    bytes[8..].copy_from_slice(&counter.to_be_bytes());
    MessageId::from_bytes(bytes)
}

/// Mints an outbound message with no clock and no entropy.
pub(crate) fn build(
    store: &Arc<dyn LeafStore>,
    identity: &Identity,
    app_id: &AppId,
    recipient: &str,
    content: String,
    now_unix_secs: u64,
    priority: MessagePriority,
) -> Result<Message> {
    let counter = next_counter(store)?;
    let sender = UserId::new(identity.address.to_string())
        .map_err(|e| LeafError::MalformedFrame(format!("own address is not a user id: {e}")))?;
    let recipient = UserId::new(recipient)
        .map_err(|e| LeafError::MalformedFrame(format!("recipient is not a user id: {e}")))?;

    let mut message = Message::from_parts(
        message_id(&identity.address, counter),
        sender,
        recipient,
        app_id.clone(),
        content,
        Timestamp::from_millis(millis(now_unix_secs)),
    );
    message.priority = priority;
    message.lamport_clock = LamportClock::from_value(counter);

    // A leaf asks for no delivery acknowledgement, because it has nothing to
    // do with one. The default is `true`, which is right for a sender that
    // holds a retry queue and settles a message against the answer; this
    // device has neither, so every acknowledgement it provoked would be a
    // frame it parses as carrying no prefix it answers and drops. On a link
    // with very little airtime that is one wasted transmission per frame sent.
    //
    // The other direction is not this crate's to decide. A phone marks its own
    // frames as needing one and a leaf emits none, so it retries until it
    // gives up. Whether a leaf peer is exempt from that machinery or owes an
    // acknowledgement is a question for the spec, which today lists neither.
    message.requires_ack = false;
    Ok(message)
}

/// Seconds to milliseconds, saturating rather than wrapping.
///
/// A device whose time source hands back something absurd gets a clamped
/// timestamp rather than a negative one. The timestamp is display metadata
/// and is not a security input, so clamping loses nothing that matters.
fn millis(now_unix_secs: u64) -> i64 {
    now_unix_secs
        .saturating_mul(1000)
        .try_into()
        .unwrap_or(i64::MAX)
}

/// Stamps a control frame with a signature and the key that made it.
///
/// The canonical payload comes from the sealed layer, which is the same
/// function the phone's producer and verifier both call, so there is no
/// second construction here to get subtly wrong. Metadata is deliberately
/// outside the signature because relays rewrite it, which is also why the
/// order of these two inserts does not matter.
pub(crate) fn sign_control_frame(identity: &Identity, message: &mut Message) -> Result<()> {
    let payload = control_signing_payload(message)?;
    let signature = identity::sign(identity, &payload)?;
    message
        .metadata
        .insert(CTRL_SIG_META_KEY.to_string(), BASE64.encode(&signature));
    message.metadata.insert(
        CTRL_PK_META_KEY.to_string(),
        BASE64.encode(identity.public.as_bytes()),
    );
    Ok(())
}

/// Verifies a control frame and returns the key that signed it.
///
/// Unsigned is a refusal, not a downgrade. Every control frame in this
/// protocol is signed, so one that is not is either an implementation that
/// skipped the step or an injection, and there is no third reading that would
/// make accepting it safe.
pub(crate) fn verify_control_frame(message: &Message) -> Result<Vec<u8>> {
    let signature = message.metadata.get(CTRL_SIG_META_KEY);
    let public_key = message.metadata.get(CTRL_PK_META_KEY);

    let (signature, public_key) =
        match (signature, public_key) {
            (Some(s), Some(p)) => (s, p),
            (None, None) => {
                return Err(LeafError::ControlFrameRefused(String::from(
                    "control frame carries no signature",
                )))
            }
            // Half the pair is worse than neither: it is a frame that was shaped
            // to look signed to something that checks only for presence.
            _ => return Err(LeafError::ControlFrameRefused(String::from(
                "control frame carries a signature without its key, or a key without its signature",
            ))),
        };

    let signature = BASE64
        .decode(signature)
        .map_err(|e| LeafError::ControlFrameRefused(format!("signature is not base64: {e}")))?;
    let public_key = BASE64
        .decode(public_key)
        .map_err(|e| LeafError::ControlFrameRefused(format!("public key is not base64: {e}")))?;

    verify_sender_derivation(message.sender.as_str(), &public_key)?;

    let payload = control_signing_payload(message)?;
    identity::verify(&public_key, &signature, &payload)?;
    Ok(public_key)
}

/// Requires the presented key to derive to the address the frame claims.
///
/// # Why an unparseable sender is an error rather than a skip
///
/// A sender that is not an address has no derivation to check, and the
/// tempting answer, pass because there is nothing to compare, hands over the
/// whole gate: an attacker claims a nickname and the check that distinguishes
/// them from its owner never runs. Refusing outright is what makes this
/// unconditional in the sense that matters, which is that there is no input
/// for which it declines to run.
pub(crate) fn verify_sender_derivation(sender: &str, public_key: &[u8]) -> Result<()> {
    let claimed = sender.parse::<Address>().map_err(|e| {
        LeafError::IdentityBinding(format!("sender '{sender}' is not an address: {e}"))
    })?;
    let derived = derive_address(public_key)?;
    if derived != claimed {
        return Err(LeafError::IdentityBinding(format!(
            "sender address mismatch: '{claimed}' claimed, key derives to '{derived}'"
        )));
    }
    Ok(())
}

/// Splits a reserved prefix off a frame's content.
///
/// Returns the body, or `None` when the content does not carry this prefix.
pub(crate) fn strip_prefix<'a>(content: &'a str, prefix: &str) -> Option<&'a str> {
    content.strip_prefix(prefix)
}
