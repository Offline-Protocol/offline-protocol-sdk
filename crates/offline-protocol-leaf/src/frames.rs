//! Minting outbound frames, and refusing inbound ones that cannot prove who
//! sent them.
//!
//! # The gate
//!
//! Every control frame in this protocol carries an Ed25519 signature in two
//! metadata keys, over a domain-separated canonical payload built from the
//! sender, the id, the recipient, the content and the frame's timestamp. A
//! leaf verifies four things and refuses on any of them:
//!
//! 1. the signature metadata is present and complete,
//! 2. the signature verifies under the key the frame presents,
//! 3. **the presented key derives to the address the frame claims to be from**,
//! 4. **the frame is not older, or further ahead of this device's clock, than
//!    the window allows**.
//!
//! The third is the one that says *who*. The first two prove a key signed
//! this; only the third proves it is the peer's key. Both halves of a
//! mismatch, and an identifier that is not an address at all, are the same
//! refusal: an identifier with no derivation to check is not a claim that
//! needs waving through, it is the bypass, and answering "acceptable" for it
//! is how an attacker skips the gate by claiming a nickname.
//!
//! The fourth says *when*, and without it the other three are a signature that
//! never expires: a frame captured off the air verifies as well on its tenth
//! delivery as on its first, and a key package carrying `session_reset` tears
//! down a live session on each one (issue 403).
//!
//! # Which signing payload a leaf verifies
//!
//! Two payloads exist: the older one, which binds the sender, the id, the
//! recipient and the content, and the freshness-bound one, which additionally
//! binds the frame's timestamp. A leaf verifies the freshness-bound payload on
//! every control frame, and the older one on exactly one: `__MLS_KEY_PKG__`.
//!
//! The exception is required rather than permitted, and the reason is not that
//! some phones are old. A sender builds the freshness-bound payload for a
//! recipient that has advertised it and the older one for a recipient that has
//! not, and capabilities arrive **in a key package**, so the first key package
//! to a peer never met is signed under the older payload no matter how new the
//! sender is. That is inherent to the negotiation rather than a gap to close
//! (ADR 0023, "First contact necessarily signs v1"). The same shape recurs
//! later: a peer whose record of this device was evicted or lost signs the
//! older payload again, because it no longer knows what this device accepts,
//! and a key package is the only frame that can re-teach it.
//!
//! So refusing the older payload here does not harden the device, it makes a
//! phone-initiated pairing impossible: the frame is refused, the device never
//! learns the phone exists, and the phone's retry ladder delivers the same
//! refusal ten times over, which reaches firmware as a run of signature
//! failures indistinguishable from an attack.
//!
//! What the exception costs is nothing an attacker wants. **A frame accepted
//! under the older payload has its `session_reset` ignored**, so the one
//! directive that destroys state still requires a stamp inside the signature,
//! and the replay closed by issue 403 stays closed. What survives is
//! capability advertisement, which this protocol treats as unauthenticated
//! hint data everywhere else. The frame's age is not judged either, because
//! the older payload leaves the timestamp outside the signature and judging it
//! would be judging a number an attacker chose.
//!
//! Every other control frame a leaf accepts is refused under the older
//! payload, and none of them needs it: a Welcome and a confirmation probe can
//! only follow this device's own key package reaching the peer, which is the
//! frame that teaches the peer to sign the newer payload.

use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use offline_protocol_core::{
    Address, AppId, LamportClock, Message, MessageId, MessagePriority, Timestamp, UserId,
};
use offline_protocol_sealed::{
    control_frame_freshness, control_signing_payload, control_signing_payload_v2, derive_address,
    Freshness, ACK_FOR_KEY, CTRL_FRESHNESS_FUTURE_MS, CTRL_PK_META_KEY, CTRL_SIG_META_KEY,
    LEAF_CTRL_FRESHNESS_PAST_MS,
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
    // The other direction is [`acknowledge`], and it goes the other way: a
    // leaf owes one, because a phone that never hears an answer retransmits.
    message.requires_ack = false;
    Ok(message)
}

/// Mints the delivery acknowledgement a leaf owes for a frame it received.
///
/// # Why a device answers at all
///
/// A phone marks its frames as needing an acknowledgement and settles them
/// against the answer. Against a device that never answered, every one of them
/// ran the full retry ladder: ten retransmissions of a sealed frame over about
/// thirteen minutes, on the link this crate exists to be careful with. Each
/// retransmission arrived here as a replay of a generation the ratchet had
/// already spent, so it was correctly refused, and firmware saw a run of
/// [`LeafError::Mls`](crate::LeafError::Mls) indistinguishable from somebody
/// replaying frames at the device on purpose. The one signal that would tell
/// an integrator they are under attack was buried under traffic the protocol
/// generated itself (issue 402).
///
/// The arithmetic is what decides it. An acknowledgement is empty content and
/// a single metadata entry; the frames it prevents are ten full sealed
/// envelopes. Answering costs a fraction of staying quiet, and it is also the
/// only way the phone's application ever learns that the command it sent to a
/// lock arrived.
///
/// # What it is not
///
/// It carries no prefix and no signature, and it is not evidence of anything.
/// A peer reads it as "the frame with this id reached its recipient" and
/// nothing more: the engine that receives it checks only that the answer comes
/// from the address the message was addressed to, which a forger who saw the
/// frame saw too. Session confirmation is a different frame with a different
/// meaning ([`prefixes::SESSION_CONFIRM_ACK`]), and this one must never be read
/// as standing in for it.
///
/// [`prefixes::SESSION_CONFIRM_ACK`]: offline_protocol_sealed::prefixes::SESSION_CONFIRM_ACK
pub(crate) fn acknowledge(
    store: &Arc<dyn LeafStore>,
    identity: &Identity,
    app_id: &AppId,
    answering: &Message,
    now_unix_secs: u64,
) -> Result<Message> {
    let mut ack = build(
        store,
        identity,
        app_id,
        answering.sender.as_str(),
        String::new(),
        now_unix_secs,
        MessagePriority::Low,
    )?;

    // The whole meaning of the frame. Without it a peer reads a blank message
    // and settles nothing, which is the failure this exists to end rather than
    // a degraded version of it.
    ack.metadata
        .insert(ACK_FOR_KEY.to_string(), answering.id.to_string());
    Ok(ack)
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
///
/// Always the freshness-bound payload, unconditionally. A phone chooses per
/// recipient, because it talks to installs that predate it; a leaf has no such
/// peer and no capability to consult, so there is nothing here to decide.
/// The frame's timestamp is inside this signature, so whatever `mint` stamped
/// is now covered: a device with a wrong clock produces frames its peer
/// refuses, which is the direction that fails safely.
pub(crate) fn sign_control_frame(identity: &Identity, message: &mut Message) -> Result<()> {
    let payload = control_signing_payload_v2(message)?;
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
///
/// `now_unix_secs` is this device's clock, and it is a parameter for the same
/// reason it is one everywhere else in this crate: a leaf has no clock of its
/// own to reach for, so the caller that has one supplies it. A device that
/// supplies a wrong one refuses its peer, which is loud; a device that was
/// allowed to skip supplying one would accept anything, which is silent.
/// Which signing payload a control frame proved itself under.
///
/// Carried out of [`verify_control_frame`] rather than inferred again, because
/// the one caller that admits the older payload has to treat what it carries
/// differently: a `session_reset` under it is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlPayload {
    /// The freshness-bound payload. The frame's timestamp is inside the
    /// signature and has been judged against this device's window.
    Fresh,
    /// The older payload. The timestamp is outside the signature, so it states
    /// nothing about when the frame was made and has not been judged.
    Undated,
}

/// Whether a frame class may carry the older payload.
///
/// Named rather than passed as a bare flag, because the call site is where
/// this has to be readable: [`Admitted`](UndatedPayload::Admitted) appears
/// once in this crate and every other control frame refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UndatedPayload {
    /// `__MLS_KEY_PKG__` alone, where the older payload is required rather
    /// than tolerated.
    Admitted,
    /// Every other control frame a leaf accepts.
    Refused,
}

/// Verifies a control frame, admitting the older payload only when the caller
/// says this frame class may carry it.
///
/// See this module's documentation for why `__MLS_KEY_PKG__` is required to
/// admit it and why the rest are required not to.
pub(crate) fn verify_control_frame(
    message: &Message,
    now_unix_secs: u64,
    undated: UndatedPayload,
) -> Result<ControlPayload> {
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

    // The freshness-bound payload first, always, so that a peer able to produce
    // it is never judged under the weaker one. Only a frame that fails it is
    // considered for the fallback, and only when its class admits one.
    let fresh_payload = control_signing_payload_v2(message)?;
    match identity::verify(&public_key, &signature, &fresh_payload) {
        Ok(()) => {
            // Only now, with the stamp proved to be the sender's own rather
            // than something a relay or an attacker wrote on the way past.
            // Judging it before the signature would be judging an
            // attacker-chosen number.
            match control_frame_freshness(
                message.timestamp.as_millis(),
                millis(now_unix_secs),
                LEAF_CTRL_FRESHNESS_PAST_MS,
                CTRL_FRESHNESS_FUTURE_MS,
            ) {
                Freshness::Fresh => Ok(ControlPayload::Fresh),
                Freshness::Stale { age_ms } => Err(LeafError::StaleControlFrame(format!(
                    "frame is {age_ms} ms old, past what this device accepts"
                ))),
                Freshness::FromTheFuture { skew_ms } => Err(LeafError::StaleControlFrame(format!(
                    "frame is stamped {skew_ms} ms ahead of this device's clock; check the \
                         clock before the peer"
                ))),
            }
        }
        Err(refusal) => {
            if undated == UndatedPayload::Refused {
                return Err(refusal);
            }
            // The older payload, which binds everything the newer one does
            // except the timestamp. Its age is deliberately not judged: the
            // stamp is outside the signature, so it is whatever the last hand
            // to touch the frame wrote there.
            let undated_payload = control_signing_payload(message)?;
            identity::verify(&public_key, &signature, &undated_payload)?;
            Ok(ControlPayload::Undated)
        }
    }
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
