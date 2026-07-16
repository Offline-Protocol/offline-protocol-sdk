//! Internal message prefix definitions and base64 utilities.

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
    /// Prefix for key package messages.
    KEY_PACKAGE = "__MLS_KEY_PKG__",
    /// Prefix for welcome messages.
    WELCOME = "__MLS_WELCOME__",
    /// Prefix for encrypted messages.
    ENCRYPTED = "__MLS_ENC__",
    /// Prefix for session confirmation probe messages.
    SESSION_CONFIRM_PROBE = "__MLS_CONFIRM_PROBE__",
    /// Prefix for session confirmation acknowledgement messages.
    SESSION_CONFIRM_ACK = "__MLS_CONFIRM_ACK__",
    /// Prefix for the MLS-encrypted session-confirm an adopter sends on adopting
    /// the peer's Welcome. Only ever travels INSIDE an `ENCRYPTED` envelope, never
    /// as a raw control message on the wire; its sole purpose is to be a
    /// group-aware decrypt so the both-create "owner" (which confirms only on
    /// `decrypt_success`) converges. Consumed on receipt — never surfaced to the app.
    SESSION_CONFIRM_ENCRYPTED = "__MLS_ENC_CONFIRM__",
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
///   silently dropped fan-out from every TOFU-pinned sender. Authentication
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
/// require signature verification + TOFU enforcement.
pub(crate) const DATA_PLANE_PREFIXES: &[&str] =
    &[internal_prefixes::ENCRYPTED, internal_prefixes::GROUP_MSG];
