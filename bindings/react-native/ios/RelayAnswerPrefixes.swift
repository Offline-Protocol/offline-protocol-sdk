//
// RelayAnswerPrefixes.swift
//
// Which synthesized relay frames must reach the core unattributed.
// Mirrors android's RelayAnswerPrefixes.kt — keep in sync.
//

import Foundation

/// The prefixes the **relay server** originates, which the bridge synthesizes
/// from a WebSocket answer rather than receiving from any peer.
///
/// Mirrors `RELAY_ANSWER_PREFIXES` in
/// `crates/offline-protocol/src/protocol/prefixes.rs`. The two lists must agree:
/// the core exempts exactly these from its unconditional control-frame
/// signature gate, because no peer sent them so no key exists to sign them.
///
/// # Why attribution breaks them
///
/// The core's exemption is deliberately narrower than the prefix — it also
/// requires the frame to carry **no transport peer identity**, which is what a
/// locally synthesized answer looks like. Passing a non-nil actor to
/// `internetMessageReceived` selects `on_data_received_from`, which sets that
/// identity, so the frame stops looking synthesized and is dropped as unsigned.
/// A legitimate relay notification then raises `UNSIGNED_CONTROL_REJECTED`.
///
/// That narrowness is doing real work and must not be widened to close this:
/// without it, any peer able to address us through the store-and-forward relay
/// could inject unsigned group state under one of these prefixes.
///
/// Note `__GROUP_MSG__` is **not** here. It is a data-plane prefix, never
/// signature-gated (MLS authenticates it after the fact), so it keeps its
/// attribution and remains the reachability signal for a relayed sender.
enum RelayAnswerPrefixes {

    static let all: Set<String> = [
        "__GROUP_CREATED__",
        "__GROUP_MEMBER_ADDED__",
        "__GROUP_MEMBER_REMOVED__",
        "__GROUP_INFO__",
        "__USER_GROUPS__",
        "__GROUP_ERROR__"
    ]

    /// Whether `prefix` names a relay answer that must be injected unattributed.
    static func isRelayAnswer(_ prefix: String) -> Bool {
        all.contains(prefix)
    }

    /// The actor a synthesized frame may be attributed to: `nil` for a relay
    /// answer, the caller's actor otherwise.
    ///
    /// Enforced here rather than trusted to each call site. The rule comes from
    /// a constant in the Rust core, and a new answer prefix injected with an
    /// actor would be silently dropped as unsigned — a failure no Swift or
    /// Kotlin test would otherwise catch, since neither side compiles the other.
    static func attributableActor(prefix: String, actorId: String?) -> String? {
        isRelayAnswer(prefix) ? nil : actorId
    }
}
