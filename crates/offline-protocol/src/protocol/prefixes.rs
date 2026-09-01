//! Internal message prefix definitions and base64 utilities.
//!
//! Six of the prefixes below are defined in `offline-protocol-sealed` and
//! named here rather than spelled out: the ones a leaf node also emits and
//! parses, with a different MLS implementation. Reservation still happens
//! here, because `INTERNAL_PREFIXES` is what refuses application content that
//! begins with one, and that array is generated from this macro invocation.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

/// Encode bytes to base64 string.
pub(crate) fn base64_encode(data: &[u8]) -> String {
    BASE64.encode(data)
}

/// Decode base64 string to bytes with a size guard against oversized payloads.
///
/// The limit is applied to the **encoded** (base64) size. Since base64 inflates
/// data by ~33%, the maximum **decoded** payload is approximately 768 KB.
pub(crate) fn base64_decode(data: &str) -> std::result::Result<Vec<u8>, String> {
    if data.len() > crate::group_mesh::MAX_BASE64_PAYLOAD_SIZE {
        return Err(format!(
            "payload too large: {} encoded bytes exceeds {} limit",
            data.len(),
            crate::group_mesh::MAX_BASE64_PAYLOAD_SIZE
        ));
    }
    BASE64.decode(data).map_err(|e| e.to_string())
}

/// Defines both the `internal_prefixes` module (named constants) and the
/// `INTERNAL_PREFIXES` array in one place, so they can never drift apart.
///
/// Adding a new control-message prefix is a single-line change — the macro
/// guarantees the constant and the injection-prevention array stay in sync.
macro_rules! define_internal_prefixes {
    ( $( $(#[$attr:meta])* $name:ident = $value:expr ),+ $(,)? ) => {
        /// Internal message prefixes for protocol messages.
        pub(crate) mod internal_prefixes {
            $( $(#[$attr])* pub const $name: &str = $value; )+
        }

        /// All internal message prefixes — used to reject user-sent messages
        /// that start with a reserved prefix via the public `send_message` /
        /// `send_message_via_transport` APIs (injection prevention).
        ///
        /// Service discovery prefixes (`__SVC_*`) from `offline-protocol-services`
        /// are appended after the macro-generated entries.
        pub(crate) const INTERNAL_PREFIXES: &[&str] = &[
            $( internal_prefixes::$name, )+
            // Service discovery and request/response prefixes.
            offline_protocol_services::SVC_MESSAGE_PREFIX,
            offline_protocol_services::SVC_DISCOVER_QUERY,
            offline_protocol_services::SVC_DISCOVER_RESPONSE,
            offline_protocol_services::SVC_REQUEST,
            offline_protocol_services::SVC_RESPONSE,
        ];
    };
}

define_internal_prefixes! {
    /// Prefix for key package messages. Defined in `offline-protocol-sealed`.
    KEY_PACKAGE = offline_protocol_sealed::prefixes::KEY_PACKAGE,
    /// Prefix for welcome messages. Defined in `offline-protocol-sealed`.
    WELCOME = offline_protocol_sealed::prefixes::WELCOME,
    /// Prefix for encrypted messages. Defined in `offline-protocol-sealed`.
    ENCRYPTED = offline_protocol_sealed::prefixes::ENCRYPTED,
    /// Prefix for session confirmation probe messages. Defined in
    /// `offline-protocol-sealed`.
    SESSION_CONFIRM_PROBE = offline_protocol_sealed::prefixes::SESSION_CONFIRM_PROBE,
    /// Prefix for session confirmation acknowledgement messages. Defined in
    /// `offline-protocol-sealed`.
    SESSION_CONFIRM_ACK = offline_protocol_sealed::prefixes::SESSION_CONFIRM_ACK,
    /// Prefix for the MLS-encrypted session-confirm an adopter sends on adopting
    /// the peer's Welcome. Only ever travels INSIDE an `ENCRYPTED` envelope, never
    /// as a raw control message on the wire; its sole purpose is to be a
    /// group-aware decrypt so the both-create "owner" (which confirms only on
    /// `decrypt_success`) converges. Consumed on receipt, never surfaced to the
    /// app. Defined in `offline-protocol-sealed`.
    SESSION_CONFIRM_ENCRYPTED = offline_protocol_sealed::prefixes::SESSION_CONFIRM_ENCRYPTED,
    /// Prefix for the sealed rich payload (`RichPayloadV1` JSON) inside a
    /// decrypted MLS plaintext. Only ever travels INSIDE an `ENCRYPTED`
    /// envelope, negotiated via `rich_versions` in the key package; listed
    /// here so user content can never impersonate a sealed rich body through
    /// the public send APIs.
    RICH_V1 = "__RICH_V1__",
    /// Prefix for replicated-document sync frames inside a decrypted MLS
    /// plaintext. Reserved ahead of the replication half of the data layer:
    /// the marker is registered before any frame uses it so that no user
    /// message sent in the meantime can ever occupy the name, which is the
    /// only window in which that collision is cheap to prevent.
    DATA_V1 = "__DATA_V1__",
    /// Prefix for connection request messages.
    CONN_REQUEST = "__CONN_REQ__",
    /// Prefix for connection accepted messages.
    CONN_ACCEPT = "__CONN_ACC__",
    /// Prefix for connection rejected messages.
    CONN_REJECT = "__CONN_REJ__",
    /// Prefix for connection cancelled messages.
    CONN_CANCEL = "__CONN_CAN__",
    /// Prefix for group created (relay).
    GROUP_CREATED = "__GROUP_CREATED__",
    /// Prefix for group message received (relay).
    GROUP_MSG = "__GROUP_MSG__",
    /// Prefix for group member added (relay).
    GROUP_MEMBER_ADDED = "__GROUP_MEMBER_ADDED__",
    /// Prefix for group member removed (relay).
    GROUP_MEMBER_REMOVED = "__GROUP_MEMBER_REMOVED__",
    /// Prefix for group info (relay).
    GROUP_INFO = "__GROUP_INFO__",
    /// Prefix for user groups list (relay).
    USER_GROUPS = "__USER_GROUPS__",
    /// Prefix for group error (relay).
    GROUP_ERROR = "__GROUP_ERROR__",
    /// Prefix for MLS-encrypted group messages.
    GROUP_MLS_MSG = "__GRP_MLS_MSG__",
    /// Prefix for MLS Welcome messages for group invites.
    GROUP_MLS_WELCOME = "__GRP_MLS_WELCOME__",
    /// Prefix for MLS Commit messages for group membership changes.
    GROUP_MLS_COMMIT = "__GRP_MLS_COMMIT__",
    /// Prefix for group leave notifications.
    GROUP_MLS_LEAVE = "__GRP_MLS_LEAVE__",
    /// Prefix for relay group registration (SDK → relay server).
    GROUP_RELAY_REGISTER = "__GRP_RELAY_REG__",
    /// Prefix for relay group broadcast (SDK → relay server fan-out).
    GROUP_RELAY_BROADCAST = "__GRP_RELAY_BCAST__",
    /// Prefix for group role change notifications.
    GROUP_ROLE_CHANGE = "__GRP_ROLE_CHG__",
    /// Prefix for group rename notifications.
    GROUP_RENAME = "__GRP_RENAME__",
    /// Prefix for presence update messages.
    PRESENCE = "__PRESENCE__",
    /// Prefix for typing indicator messages.
    TYPING_INDICATOR = "__TYPING__",
    /// Prefix for read receipt messages.
    READ_RECEIPT = "__READ_RECEIPT__",
}

/// Data-plane prefixes that are **excluded** from the security gate.
///
/// These prefixes rely on MLS for authentication (not Ed25519 control-message
/// signing). Listing them here rather than maintaining a separate
/// `SECURITY_GATED_PREFIXES` array ensures that any new prefix added to
/// `INTERNAL_PREFIXES` is automatically security-gated unless explicitly
/// excluded here.
///
/// - `ENCRYPTED` (`__MLS_ENC__`): 1:1 MLS envelopes, sent via `send_message`
///   and never signed outbound.
/// - `GROUP_MSG` (`__GROUP_MSG__`): the relay's group fan-out. The relay
///   re-emits it per member from only `{group_id, sender, content}`, so the
///   bridge-rebuilt frame is structurally unsigned — Ed25519-gating it
///   silently dropped fan-out from every sender. Authentication
///   happens after the gate instead: `handle_relay_group_message_with_mls`
///   MLS-decrypts and binds the wire-claimed sender to the MLS-authenticated
///   sender (`SenderIdentityMismatch` → rejected), and plaintext naming an
///   MLS-secured group is dropped as spoofing. Residual: groups with no MLS
///   state accept unauthenticated plaintext on this prefix — identical to
///   the pre-gate legacy behavior for unpinned senders, and unreachable in
///   deployments where every group is MLS.
///
/// **Maintenance note:** Only add prefixes here if their handler enforces
/// MLS authentication. All other internal prefixes are control-plane and
/// require signature verification + sender-address derivation.
pub(crate) const DATA_PLANE_PREFIXES: &[&str] =
    &[internal_prefixes::ENCRYPTED, internal_prefixes::GROUP_MSG];

/// Prefixes the **relay server** originates, which therefore cannot carry a
/// peer signature.
///
/// These are not messages any peer transmitted. The relay answers over its
/// WebSocket and the bridge *synthesizes* a frame from that answer
/// (`injectGroupInternalMessage` in `InternetManager.{swift,kt}`), with a
/// placeholder `sender` when the answer names no actor. There is no private key
/// anywhere in that path, so requiring a signature would drop every one of them
/// — taking group registration (and with it the `relay_synced` gate that group
/// broadcast depends on), relay member add/remove, group info, the user's group
/// list, and relay error reporting with it.
///
/// This is the same situation `GROUP_MSG` is in, and it is listed separately
/// rather than added to [`DATA_PLANE_PREFIXES`] because the reason differs and
/// the two must not be conflated: a data-plane frame is authenticated *later*
/// by MLS, whereas these are not authenticated by this SDK at all.
///
/// # What actually protects them, and what does not
///
/// Two things, neither of them a signature:
///
/// 1. The bridge restricts these prefixes to the relay socket
///    (`RelayControlOpTranslator`), so a mesh peer cannot deliver a crafted
///    `__GROUP_CREATED__` through the ordinary message path.
/// 2. The exemption here is narrower than the prefix: it applies only to a
///    frame that arrived on [`TransportType::Internet`] carrying no transport
///    peer identity — the shape a locally synthesized relay answer has. A peer
///    frame on a mesh transport, or one carrying a carrier identity, is still
///    required to be signed, so nothing on the peer-to-peer path is weakened by
///    this list.
///
/// **Residual, stated plainly:** anything able to inject on the relay ingest
/// path can forge these frames. That is the pre-existing relay-trust surface,
/// unchanged by this work — it is exactly what these frames were exposed to
/// before control traffic became signature-gated. Closing it means moving relay
/// answers off the message plane and onto dedicated FFI entry points, the way
/// `internet_group_report_received` already handles the group delivery report;
/// that is deliberately out of scope here and left as the follow-up.
///
/// **Maintenance note:** this list is mirrored, by hand, in three places that
/// no single compiler ever sees together — here, `RelayAnswerPrefixes.swift`,
/// and `RelayAnswerPrefixes.kt`. Each bridge pins its own copy against the same
/// literals ([`relay_answer_prefixes_are_pinned`] does it for this one), because
/// a prefix present in one list and absent from another fails **silently**: the
/// bridge injects the answer unattributed, this list declines to exempt it, and
/// the frame is dropped as unsigned with no peer at fault. Edit all three.
pub(crate) const RELAY_ANSWER_PREFIXES: &[&str] = &[
    internal_prefixes::GROUP_CREATED,
    internal_prefixes::GROUP_MEMBER_ADDED,
    internal_prefixes::GROUP_MEMBER_REMOVED,
    internal_prefixes::GROUP_INFO,
    internal_prefixes::USER_GROUPS,
    internal_prefixes::GROUP_ERROR,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The membership of [`RELAY_ANSWER_PREFIXES`], pinned to literals.
    ///
    /// Written out rather than derived, for the same reason the bridges write
    /// theirs out: this list is one of three hand-maintained copies, and a test
    /// that recomputed it from the constant would agree with any edit — which
    /// is precisely the failure mode. The literals here are the contract the
    /// two bridge lists are also pinned against, so a divergence in any one of
    /// the three now fails a test in its own language.
    ///
    /// Dropping an entry is the dangerous direction and the reason this test
    /// exists: the bridge would keep injecting that answer unattributed, the
    /// gate would refuse it as unsigned, and the visible symptom would be a
    /// relay feature quietly not working (for `__USER_GROUPS__`, group sync;
    /// for `__GROUP_CREATED__`, the `relay_synced` gate group broadcast rides
    /// on) with an `UNSIGNED_CONTROL_REJECTED` warning naming the relay.
    #[test]
    fn relay_answer_prefixes_are_pinned() {
        assert_eq!(
            RELAY_ANSWER_PREFIXES,
            [
                "__GROUP_CREATED__",
                "__GROUP_MEMBER_ADDED__",
                "__GROUP_MEMBER_REMOVED__",
                "__GROUP_INFO__",
                "__USER_GROUPS__",
                "__GROUP_ERROR__",
            ],
            "the relay-answer exemption list changed — update RelayAnswerPrefixes.swift \
             and RelayAnswerPrefixes.kt to match, or the bridges and the gate will \
             disagree silently"
        );
    }

    /// The two exemption lists must stay disjoint.
    ///
    /// They are different mechanisms with different post-conditions — a
    /// data-plane frame is authenticated later by MLS, a relay answer is not
    /// authenticated by this SDK at all — and the doc on each says so. Listing a
    /// prefix in both would make the narrow relay exemption (Internet ingest,
    /// unattributed) unreachable for it, since `is_security_gated_prefix`
    /// already excludes the data plane before the gate runs, so the three
    /// conditions would silently stop applying.
    #[test]
    fn the_two_exemption_lists_do_not_overlap() {
        for relay in RELAY_ANSWER_PREFIXES {
            assert!(
                !DATA_PLANE_PREFIXES.contains(relay),
                "'{}' is exempt twice, by two different rules",
                relay
            );
        }
    }

    /// Every exempt prefix must be an internal prefix, or it exempts nothing.
    ///
    /// The gate only consults [`RELAY_ANSWER_PREFIXES`] for content that
    /// `is_security_gated_prefix` already matched, which requires membership in
    /// [`INTERNAL_PREFIXES`]. A typo'd entry here is therefore not a widened
    /// hole — it is dead text, and the answer it was meant to exempt gets
    /// dropped as unsigned instead.
    #[test]
    fn every_exempt_prefix_is_an_internal_prefix() {
        for relay in RELAY_ANSWER_PREFIXES {
            assert!(
                INTERNAL_PREFIXES.contains(relay),
                "'{}' is not an internal prefix, so exempting it does nothing",
                relay
            );
        }
    }
}

/// The reserved prefix registry, held against the chapter that publishes it.
///
/// The chapter is `docs/spec/control-messages.md`, whose registry tables are
/// the list a second implementation reserves from. Nothing else compares the
/// two, and the failure is silent in the direction that matters: a prefix this
/// build reserves but the chapter omits is a prefix another implementation
/// happily lets application text impersonate, and the frame it forges is
/// indistinguishable from a real one at the receiver.
///
/// This is a membership check in both directions rather than a byte vector,
/// because a prefix registry has no bytes to pin: what it has is a set, and the
/// bug is always a missing element.
#[cfg(test)]
mod spec_registry {
    use super::*;

    /// The chapter, or `None` where the repo tree is absent.
    ///
    /// Read at runtime rather than with `include_str!` because the chapter
    /// lives outside the package root: `cargo package` carries `tests/` but
    /// cannot carry `docs/`, so compiling the path in would leave the published
    /// crate's tests unable to build at all.
    fn chapter() -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/spec/control-messages.md");
        std::fs::read_to_string(&path).ok().or_else(|| {
            eprintln!("spec tree not present, skipping the control-messages registry checks");
            None
        })
    }

    /// Every prefix this build reserves is published.
    ///
    /// Without this, a prefix added to the macro and forgotten in the chapter
    /// is reserved here and unreserved everywhere else.
    #[test]
    fn the_chapter_publishes_every_prefix_this_build_reserves() {
        let Some(text) = chapter() else { return };

        for prefix in INTERNAL_PREFIXES {
            assert!(
                text.contains(prefix),
                "the control-messages chapter does not publish {prefix}, so a \
                 second implementation would not reserve it and application \
                 text could impersonate that frame"
            );
        }
    }

    /// Every prefix the chapter publishes is reserved by this build.
    ///
    /// The other direction, and the one that catches a rename: a chapter entry
    /// with no constant behind it means this build accepts application content
    /// that every conforming peer refuses, which is the asymmetry that turns
    /// into an injection vector at exactly one end of a conversation.
    ///
    /// `__SVC_` is excluded from the token scan by construction rather than by
    /// exception: it bounds a namespace instead of naming a frame, so it is
    /// checked as a reserved entry above and not required to round-trip as a
    /// frame tag here.
    #[test]
    fn this_build_reserves_every_prefix_the_chapter_publishes() {
        let Some(text) = chapter() else { return };

        let mut published: Vec<String> = Vec::new();
        for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if token.len() > 4 && token.starts_with("__") && token.ends_with("__") {
                if !published.iter().any(|p| p == token) {
                    published.push(token.to_string());
                }
            }
        }

        assert!(
            published.len() > 20,
            "the registry scan found only {} prefixes, so it is no longer \
             reading the chapter's tables and would pass against anything",
            published.len()
        );

        for prefix in &published {
            assert!(
                INTERNAL_PREFIXES.contains(&prefix.as_str()),
                "the chapter publishes {prefix} but this build does not reserve \
                 it, so application content beginning with it is accepted here \
                 and refused by every conforming peer"
            );
        }
    }

    /// The two exemption classes stay disjoint, and the chapter says so.
    ///
    /// A prefix in both lists would make the narrow relay conditions
    /// unreachable for it, because the data-plane exclusion is consulted first.
    #[test]
    fn the_chapter_states_the_exemption_classes_are_disjoint() {
        let Some(text) = chapter() else { return };

        assert!(
            text.contains("The two exemption lists must stay disjoint"),
            "the chapter no longer states the disjointness requirement that \
             `relay_and_data_plane_exemptions_are_disjoint` enforces"
        );
        for relay in RELAY_ANSWER_PREFIXES {
            assert!(
                !DATA_PLANE_PREFIXES.contains(relay),
                "{relay} is in both exemption lists"
            );
        }
    }
}
