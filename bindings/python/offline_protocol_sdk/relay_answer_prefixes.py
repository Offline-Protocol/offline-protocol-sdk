"""Which synthesized relay frames must reach the core unattributed.

A port of ``RelayAnswerPrefixes.swift`` and ``RelayAnswerPrefixes.kt``.

These are the prefixes the **relay server** originates. The bridge
synthesizes a frame from a WebSocket answer rather than receiving one from a
peer, so no key exists to sign it. The core exempts exactly these from its
control-frame signature gate (``RELAY_ANSWER_PREFIXES`` in
``crates/offline-protocol/src/protocol/prefixes.rs``), and the four copies
must agree: the core, the Swift bridge, the Kotlin bridge and this module.

Why attribution breaks them: the core's exemption is narrower than the
prefix. It also requires the frame to carry **no transport peer identity**,
which is what a locally synthesized answer looks like. Passing a non-empty
``sender_id`` to ``internet_message_received`` sets that identity, so the
frame stops looking synthesized and is dropped as unsigned. The caller sees a
successful inject and the answer never takes effect. That narrowness is doing
real work and must not be widened to close this: without it, any peer able to
address us through the relay could inject unsigned group state.

``__GROUP_MSG__`` is **not** here. It is a data-plane prefix, never
signature-gated (MLS authenticates it afterwards), so it keeps its
attribution and remains the reachability signal for a relayed sender.

``test_relay_answer_prefixes.py`` pins the set as literals (contract C5 in
``docs/bridges/README.md``). A test that recomputed it from this constant
would agree with any edit, which is the failure it exists to catch.
"""

from __future__ import annotations

RELAY_ANSWER_PREFIXES: frozenset[str] = frozenset(
    {
        "__GROUP_CREATED__",
        "__GROUP_MEMBER_ADDED__",
        "__GROUP_MEMBER_REMOVED__",
        "__GROUP_INFO__",
        "__USER_GROUPS__",
        "__GROUP_ERROR__",
    }
)


def is_relay_answer(prefix: str) -> bool:
    """Whether ``prefix`` names a relay answer that must be injected unattributed.

    Matched whole, not as a prefix of a prefix: whether a content string that
    starts with an exempt prefix is admitted is decided in the core, against
    the frame's transport and attribution.
    """
    return prefix in RELAY_ANSWER_PREFIXES


def attributable_actor(prefix: str, actor: str | None) -> str | None:
    """The actor a synthesized frame may carry: ``None`` for a relay answer.

    Enforced here rather than trusted to each call site. The rule comes from
    a constant in the Rust core, and a new answer injected with an actor is
    dropped as unsigned with no error anywhere.
    """
    return None if is_relay_answer(prefix) else actor
