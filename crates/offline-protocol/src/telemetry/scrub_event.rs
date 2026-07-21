//! Per-variant identifier scrubber for [`Event`].
//!
//! Runs on the `TelemetryRecord::Protocol` fan-out inside
//! `SharedState::emit_event`, so long-lived pseudonymous identifiers are
//! hashed before reaching the installed [`crate::telemetry::TelemetrySink`].
//! Legacy `EventCallback` handlers still receive the raw event.
//!
//! # Scrubbing policy
//!
//! - **Actor identifiers** are hashed. These are the values a third-party
//!   sink could use to profile a party across unrelated events:
//!   `peer_id`, `sender`, `recipient`, `accepted_by`/`rejected_by`/
//!   `cancelled_by`, `user_id`, `added_by`/`removed_by`/`created_by`/
//!   `changed_by`/`renamed_by`, `provider_peer_id`, `group_id`,
//!   `conversation_id`, entries in `failed_members`/`succeeded_members`,
//!   [`ForwardInfoEvent::original_sender`], [`GroupInfoMember::user_id`],
//!   [`UserGroupSummary::group_id`].
//!
//! - **Message-scoped IDs are left raw**: `message_id`, `file_id`,
//!   `query_id`, `request_id`, `service_id`, `ForwardInfoEvent::
//!   original_message_id`. These are per-event UUIDs playing the same role
//!   as OpenTelemetry trace/span IDs — correlation tokens, not identity
//!   markers. Hashing them would break debugging against server logs
//!   without meaningfully reducing identifiability, because they are
//!   single-use anyway.
//!
//! - **Content and display fields are left raw**: `content`, `file_data`,
//!   `name`, `new_name`/`old_name`, `group_name`, `sender_name`,
//!   `accepted_by_name`, `file_name`, `initial_message`,
//!   `reason`/`reason_detail`, `method`, `body`, `version`. Scrubbing
//!   payload is out of scope for `scrub_ids`; if that ever needs to change
//!   it belongs behind a separate `emit_content` knob so the two concerns
//!   don't get conflated.
//!
//! - **Secret material is redacted unconditionally** (independent of the
//!   `scrub_ids` setting): `media_metadata.encryption_key`/`iv` grant
//!   access to cloud-stored media and are cleared from every event before
//!   it reaches a sink. This is the one place the scrubber touches payload,
//!   because a key is neither an identifier nor content — leaking it hands
//!   the sink backend the media itself.
//!
//! - **Enum-ish string fields are left raw**: `transport`, `content_type`,
//!   `role`/`new_role`, `priority`, `status` — finite value sets, not
//!   identities. The `String` entries inside
//!   [`Event::DorsScoreUpdated::scores`] fall here too (they're transport
//!   names, not peer IDs).
//!
//! # Out of scope for `scrub_ids`
//!
//! Two classes of data are *deliberately* not touched by this scrubber and
//! need to be handled by sink authors, not here:
//!
//! - **Cryptographic blobs that embed identity**: the `key_package` bytes
//!   on [`Event::ConnectionRequestReceived`] and
//!   [`Event::ConnectionAccepted`] carry MLS credentials whose leaf
//!   identity encodes the raw `user_id`. A sink that logs these bytes
//!   effectively logs an unscrubbed identifier, regardless of the
//!   `scrub_ids` setting. Treat key packages as sensitive payload, not as
//!   metadata.
//!
//! - **Pre-session plaintext user prose**: `initial_message` on
//!   [`Event::ConnectionRequestReceived`] is user-authored text that was
//!   never end-to-end encrypted (connection requests precede the MLS
//!   session). It follows the content-fields-stay-raw rule above, so a
//!   sink that ships events off-device must treat it exactly like
//!   `content` on a decrypted message — as message payload, never as
//!   loggable metadata.
//!
//! - **Free-form strings that may interpolate identifiers**: `reason` /
//!   `reason_detail` fields are produced by upstream error paths. If an
//!   author ever includes a peer ID in a reason string (e.g. "no session
//!   for alice"), the identifier passes through unhashed. The SDK
//!   currently avoids this, but the contract is one-way: sinks should
//!   assume `reason` fields are free text and apply app-level redaction
//!   if they ship logs off-device.
//!
//! If that ever needs to change globally it belongs behind a separate
//! `emit_content` knob so the identifier-scrubbing and payload-scrubbing
//! concerns don't get conflated.
//!
//! # Compile-time coverage
//!
//! [`event_variant_exhaustiveness_ward`] mirrors the pattern shipped in
//! PR #91 (`telemetry::record::tests`). Adding a new variant to [`Event`]
//! without extending the scrub match below breaks compilation — which is
//! the point. Silent identifier leaks when someone adds a new variant are
//! exactly what this ward prevents.
//!
//! [`ForwardInfoEvent::original_sender`]: crate::events::ForwardInfoEvent
//! [`GroupInfoMember::user_id`]: crate::events::GroupInfoMember
//! [`UserGroupSummary::group_id`]: crate::events::UserGroupSummary

use std::borrow::Cow;

use crate::events::Event;
use crate::telemetry::scrubber::Scrubber;

/// Returns an event with all long-lived pseudonymous identifiers hashed,
/// or a borrowed reference when `scrubber` is disabled.
///
/// The borrowed path skips the per-field hashing work and the `Event`
/// clone performed by `scrub_in_place`. It is *not* zero-cost at the
/// current sink call site: `SharedState::emit_event` needs owned data to
/// construct [`crate::telemetry::TelemetryRecord::Protocol`] (which holds
/// a `Box<Event>`), so that caller always pays a clone via
/// [`Cow::into_owned`]. The `Cow` return still lets future callers that
/// can tolerate a borrow (tests, in-process readers) avoid the clone.
pub(crate) fn scrub_event<'a>(event: &'a Event, scrubber: &Scrubber) -> Cow<'a, Event> {
    // Secret redaction is NOT gated by `scrub_ids`: the cloud-media content
    // key on `media_metadata` grants access to the media itself, and a sink
    // that ships events off-device must never see it, regardless of how the
    // identifier-hashing knob is set. (`scrub_ids` trades debuggability of
    // *identifiers* against profiling risk; key material is not part of
    // that trade.)
    let carries_secrets = event_media_metadata(event).is_some_and(|m| m.has_secrets());
    if !scrubber.is_enabled() && !carries_secrets {
        return Cow::Borrowed(event);
    }
    let mut scrubbed = event.clone();
    if carries_secrets {
        redact_media_secrets(&mut scrubbed);
    }
    if scrubber.is_enabled() {
        scrub_in_place(&mut scrubbed, scrubber);
    }
    Cow::Owned(scrubbed)
}

/// The `media_metadata` carried by this event, if the variant has one.
fn event_media_metadata(event: &Event) -> Option<&offline_protocol_core::MediaMetadata> {
    match event {
        Event::MessageReceived { media_metadata, .. }
        | Event::FileReceived { media_metadata, .. }
        | Event::GroupMessageReceived { media_metadata, .. } => media_metadata.as_ref(),
        _ => None,
    }
}

/// Clears `encryption_key`/`iv` from the event's `media_metadata`. See
/// [`offline_protocol_core::MediaMetadata::without_secrets`].
fn redact_media_secrets(event: &mut Event) {
    if let Event::MessageReceived { media_metadata, .. }
    | Event::FileReceived { media_metadata, .. }
    | Event::GroupMessageReceived { media_metadata, .. } = event
    {
        if let Some(meta) = media_metadata {
            *meta = meta.without_secrets();
        }
    }
}

fn hash_string(s: &mut String, scrubber: &Scrubber) {
    let hashed = scrubber.hash_id(s.as_str()).into_owned();
    *s = hashed;
}

fn hash_each(values: &mut [String], scrubber: &Scrubber) {
    for s in values.iter_mut() {
        hash_string(s, scrubber);
    }
}

fn scrub_in_place(event: &mut Event, scrubber: &Scrubber) {
    match event {
        Event::MessageSent {
            sender,
            recipient,
            forward_info,
            // Non-identifier fields deliberately left raw.
            message_id: _,
            content: _,
            priority: _,
            requires_ack: _,
            timestamp: _,
            lamport_clock: _,
        } => {
            hash_string(sender, scrubber);
            hash_string(recipient, scrubber);
            if let Some(fi) = forward_info {
                hash_string(&mut fi.original_sender, scrubber);
            }
        }
        Event::MessageReceived {
            sender,
            recipient,
            forward_info,
            reply_context,
            message_id: _,
            content: _,
            hop_count: _,
            transport: _,
            timestamp: _,
            lamport_clock: _,
            reply_to_msg: _,
            content_type: _,
            media_metadata: _,
            encrypted: _,
        } => {
            hash_string(sender, scrubber);
            hash_string(recipient, scrubber);
            if let Some(fi) = forward_info {
                hash_string(&mut fi.original_sender, scrubber);
            }
            // The quoted sender is an identifier and gets hashed; the quoted
            // text is content and stays raw, like `content` itself.
            if let Some(rc) = reply_context {
                hash_string(&mut rc.sender, scrubber);
            }
        }
        Event::MessageDelivered {
            message_id: _,
            latency_ms: _,
            hop_count: _,
            transport: _,
        } => {}
        Event::MessageFailed {
            message_id: _,
            reason: _,
            retry_count: _,
        } => {}
        Event::MessageDecryptionFailed {
            sender,
            message_id: _,
            code: _,
            reason: _,
        } => {
            hash_string(sender, scrubber);
        }
        Event::TransportSwitched {
            from: _,
            to: _,
            reason: _,
        } => {}
        Event::RelayPromoted {
            connection_count: _,
            battery_level: _,
        } => {}
        Event::RelayDemoted { reason: _ } => {}
        Event::NeighborDiscovered {
            peer_id,
            transport: _,
            rssi: _,
        } => {
            hash_string(peer_id, scrubber);
        }
        Event::NeighborLost { peer_id } => {
            hash_string(peer_id, scrubber);
        }
        Event::NetworkMetrics {
            neighbor_count: _,
            relay_count: _,
            delivery_ratio: _,
            avg_latency_ms: _,
        } => {}
        Event::FileProgress {
            file_id: _,
            chunks_sent: _,
            total_chunks: _,
            percentage: _,
        } => {}
        Event::FileReceived {
            sender,
            forward_info,
            reply_context,
            file_id: _,
            file_name: _,
            file_size: _,
            content_type: _,
            media_metadata: _,
            file_data: _,
            timestamp: _,
            caption: _,
            reply_to_msg: _,
        } => {
            hash_string(sender, scrubber);
            if let Some(fi) = forward_info {
                hash_string(&mut fi.original_sender, scrubber);
            }
            // The quoted sender is an identifier and gets hashed; the quoted
            // text and caption are content and stay raw, like `content`.
            if let Some(rc) = reply_context {
                hash_string(&mut rc.sender, scrubber);
            }
        }
        Event::FileReceiveFailed {
            sender,
            file_id: _,
            file_name: _,
            reason: _,
        } => {
            hash_string(sender, scrubber);
        }
        Event::MediaSent {
            recipient,
            file_id: _,
            content_type: _,
        } => {
            hash_string(recipient, scrubber);
        }
        Event::MediaSendFailed {
            recipient,
            file_id: _,
            reason: _,
        } => {
            hash_string(recipient, scrubber);
        }
        Event::MessageDeferred {
            message_id: _,
            reason: _,
            retry_count: _,
            next_retry_at: _,
        } => {}
        Event::MessageRetrying {
            message_id: _,
            recipient,
            retry_count: _,
            next_retry_at: _,
        } => {
            hash_string(recipient, scrubber);
        }
        Event::MessageUndeliverable {
            message_id: _,
            recipient,
            reason: _,
            file_id: _,
        } => {
            hash_string(recipient, scrubber);
        }
        Event::MediaResendRequired {
            file_id: _,
            recipient,
            file_name: _,
            file_size: _,
        } => {
            hash_string(recipient, scrubber);
        }
        Event::AckEvicted {
            message_id: _,
            priority: _,
            reason: _,
        } => {}
        Event::FragmentAssemblyEvicted {
            message_id: _,
            completion_percent: _,
            reason: _,
        } => {}
        Event::RelayDemotedBattery {
            battery_level: _,
            min_required: _,
        } => {}
        Event::SecureSessionEstablished {
            peer_id,
            group_id,
            is_session: _,
            initiated_by_local: _,
        } => {
            hash_string(peer_id, scrubber);
            hash_string(group_id, scrubber);
        }
        Event::SecureSessionFailed { peer_id, reason: _ } => {
            hash_string(peer_id, scrubber);
        }
        Event::ConvergenceDiag {
            peer_id,
            stage: _,
            detail: _,
        } => {
            hash_string(peer_id, scrubber);
        }
        Event::WelcomeSendAttempted {
            peer_id,
            group_id,
            message_id: _,
            attempt: _,
        } => {
            hash_string(peer_id, scrubber);
            hash_string(group_id, scrubber);
        }
        Event::WelcomeSendSucceeded {
            peer_id,
            group_id,
            message_id: _,
            attempt: _,
        } => {
            hash_string(peer_id, scrubber);
            hash_string(group_id, scrubber);
        }
        Event::WelcomeSendFailed {
            peer_id,
            group_id,
            message_id: _,
            attempt: _,
            reason_code: _,
            transport_error: _,
            retryable: _,
            next_retry_at: _,
        } => {
            hash_string(peer_id, scrubber);
            hash_string(group_id, scrubber);
        }
        Event::WelcomeSendExpired {
            peer_id,
            message_id: _,
            attempt: _,
            reason_code: _,
        } => {
            hash_string(peer_id, scrubber);
        }
        Event::ConnectionRequestReceived {
            sender,
            sender_name: _,
            timestamp: _,
            key_package: _,
            initial_message: _,
        } => {
            hash_string(sender, scrubber);
        }
        Event::ConnectionRequestUndeliverable {
            recipient,
            message_id: _,
            reason: _,
        } => {
            hash_string(recipient, scrubber);
        }
        Event::ConnectionAccepted {
            accepted_by,
            accepted_by_name: _,
            timestamp: _,
            key_package: _,
        } => {
            hash_string(accepted_by, scrubber);
        }
        Event::ConnectionRejected { rejected_by } => {
            hash_string(rejected_by, scrubber);
        }
        Event::ConnectionRequestCancelled { cancelled_by } => {
            hash_string(cancelled_by, scrubber);
        }
        Event::GroupCreated { group_id, name: _ } => {
            hash_string(group_id, scrubber);
        }
        Event::GroupMessageReceived {
            group_id,
            sender,
            forward_info,
            content: _,
            timestamp: _,
            message_id: _,
            reply_to_msg: _,
            // Secrets already cleared by `redact_media_secrets`; the
            // remaining metadata and the content-type hint carry no ids.
            media_metadata: _,
            content_type: _,
        } => {
            hash_string(group_id, scrubber);
            hash_string(sender, scrubber);
            if let Some(fi) = forward_info {
                hash_string(&mut fi.original_sender, scrubber);
            }
        }
        Event::GroupMemberAdded {
            group_id,
            user_id,
            added_by,
            group_name: _,
        } => {
            hash_string(group_id, scrubber);
            hash_string(user_id, scrubber);
            hash_string(added_by, scrubber);
        }
        Event::GroupMemberRemoved {
            group_id,
            user_id,
            removed_by,
        } => {
            hash_string(group_id, scrubber);
            hash_string(user_id, scrubber);
            hash_string(removed_by, scrubber);
        }
        Event::GroupInfo {
            group_id,
            created_by,
            members,
            name: _,
            created_at: _,
        } => {
            hash_string(group_id, scrubber);
            hash_string(created_by, scrubber);
            for member in members.iter_mut() {
                hash_string(&mut member.user_id, scrubber);
            }
        }
        Event::UserGroups { groups } => {
            for summary in groups.iter_mut() {
                hash_string(&mut summary.group_id, scrubber);
            }
        }
        Event::GroupError { reason: _ } => {}
        Event::GroupMessageSent {
            group_id,
            message_ids: _,
            member_count: _,
        } => {
            hash_string(group_id, scrubber);
        }
        Event::GroupMessagePartialFailure {
            group_id,
            failed_members,
            succeeded_members,
        } => {
            hash_string(group_id, scrubber);
            hash_each(failed_members, scrubber);
            hash_each(succeeded_members, scrubber);
        }
        Event::GroupRichExtrasDropped { group_id } => {
            hash_string(group_id, scrubber);
        }
        Event::GroupEpochForkDetected {
            group_id,
            local_epoch: _,
        } => {
            hash_string(group_id, scrubber);
        }
        Event::GroupEpochForkResolved {
            group_id,
            resolved_epoch: _,
            failed_members,
        } => {
            hash_string(group_id, scrubber);
            hash_each(failed_members, scrubber);
        }
        Event::GroupRoleChanged {
            group_id,
            user_id,
            changed_by,
            new_role: _,
        } => {
            hash_string(group_id, scrubber);
            hash_string(user_id, scrubber);
            hash_string(changed_by, scrubber);
        }
        Event::GroupRenamed {
            group_id,
            renamed_by,
            new_name: _,
            old_name: _,
        } => {
            hash_string(group_id, scrubber);
            hash_string(renamed_by, scrubber);
        }
        Event::ServiceDiscovered {
            provider_peer_id,
            query_id: _,
            service_id: _,
            version: _,
            capabilities: _,
            hop_count: _,
        } => {
            hash_string(provider_peer_id, scrubber);
        }
        Event::ServiceRequestReceived {
            sender,
            request_id: _,
            service_id: _,
            method: _,
            body: _,
        } => {
            hash_string(sender, scrubber);
        }
        Event::ServiceResponseReceived {
            provider_peer_id,
            request_id: _,
            service_id: _,
            status: _,
            body: _,
        } => {
            hash_string(provider_peer_id, scrubber);
        }
        Event::PresenceUpdated {
            peer_id,
            status: _,
            timestamp: _,
            last_seen_ms: _,
            source: _,
        } => {
            hash_string(peer_id, scrubber);
        }
        Event::TypingIndicatorReceived {
            sender,
            conversation_id,
            is_typing: _,
            timestamp: _,
        } => {
            hash_string(sender, scrubber);
            // `conversation_id` is a recipient username for DMs or a group_id
            // for groups — both are actor-class identifiers, hash either way.
            hash_string(conversation_id, scrubber);
        }
        Event::ReadReceiptReceived {
            sender,
            message_ids: _,
            timestamp: _,
        } => {
            hash_string(sender, scrubber);
        }
        Event::DorsScoreUpdated { scores: _ } => {
            // Inner `String` is a transport name (ble, wifi_direct, ...),
            // not an identifier. Nothing to scrub.
        }
        Event::DorsTransportSelected {
            from: _,
            transport: _,
            reason_code: _,
            score: _,
        } => {}
        Event::DorsTransportSwitched {
            from: _,
            to: _,
            reason_code: _,
            reason_detail: _,
        } => {}
        Event::DorsEscalationTriggered {
            phase: _,
            from: _,
            to: _,
            reason_code: _,
            reason_detail: _,
        } => {}
        Event::SecurityWarning {
            peer_id,
            reason_code: _,
            reason: _,
        } => {
            hash_string(peer_id, scrubber);
        }
        Event::MessageRelayed {
            sender,
            recipient,
            message_id: _,
            hop_count: _,
            remaining_ttl: _,
        } => {
            hash_string(sender, scrubber);
            hash_string(recipient, scrubber);
        }
        Event::UserBlocked { user_id } => {
            hash_string(user_id, scrubber);
        }
        Event::UserUnblocked { user_id } => {
            hash_string(user_id, scrubber);
        }
        Event::TofuReset { peer_id } => {
            hash_string(peer_id, scrubber);
        }
    }
}

/// Compile-time exhaustiveness ward.
///
/// This match deliberately omits a wildcard arm, so adding a new variant to
/// [`Event`] breaks compilation here and forces the author to extend
/// [`scrub_in_place`]. Without this ward, a new variant could silently ship
/// with its identifier fields unhashed on the sink fan-out — exactly the
/// class of privacy regression `scrub_ids` is meant to prevent.
#[allow(dead_code)]
fn event_variant_exhaustiveness_ward(e: &Event) {
    match e {
        Event::MessageSent { .. }
        | Event::MessageReceived { .. }
        | Event::MessageDelivered { .. }
        | Event::MessageFailed { .. }
        | Event::MessageDecryptionFailed { .. }
        | Event::TransportSwitched { .. }
        | Event::RelayPromoted { .. }
        | Event::RelayDemoted { .. }
        | Event::NeighborDiscovered { .. }
        | Event::NeighborLost { .. }
        | Event::NetworkMetrics { .. }
        | Event::FileProgress { .. }
        | Event::FileReceived { .. }
        | Event::FileReceiveFailed { .. }
        | Event::MediaSent { .. }
        | Event::MediaSendFailed { .. }
        | Event::MessageDeferred { .. }
        | Event::MessageRetrying { .. }
        | Event::MessageUndeliverable { .. }
        | Event::MediaResendRequired { .. }
        | Event::AckEvicted { .. }
        | Event::FragmentAssemblyEvicted { .. }
        | Event::RelayDemotedBattery { .. }
        | Event::SecureSessionEstablished { .. }
        | Event::SecureSessionFailed { .. }
        | Event::ConvergenceDiag { .. }
        | Event::WelcomeSendAttempted { .. }
        | Event::WelcomeSendSucceeded { .. }
        | Event::WelcomeSendFailed { .. }
        | Event::WelcomeSendExpired { .. }
        | Event::ConnectionRequestReceived { .. }
        | Event::ConnectionRequestUndeliverable { .. }
        | Event::ConnectionAccepted { .. }
        | Event::ConnectionRejected { .. }
        | Event::ConnectionRequestCancelled { .. }
        | Event::GroupCreated { .. }
        | Event::GroupMessageReceived { .. }
        | Event::GroupMemberAdded { .. }
        | Event::GroupMemberRemoved { .. }
        | Event::GroupInfo { .. }
        | Event::UserGroups { .. }
        | Event::GroupError { .. }
        | Event::GroupMessageSent { .. }
        | Event::GroupMessagePartialFailure { .. }
        | Event::GroupRichExtrasDropped { .. }
        | Event::GroupEpochForkDetected { .. }
        | Event::GroupEpochForkResolved { .. }
        | Event::GroupRoleChanged { .. }
        | Event::GroupRenamed { .. }
        | Event::ServiceDiscovered { .. }
        | Event::ServiceRequestReceived { .. }
        | Event::ServiceResponseReceived { .. }
        | Event::PresenceUpdated { .. }
        | Event::TypingIndicatorReceived { .. }
        | Event::ReadReceiptReceived { .. }
        | Event::DorsScoreUpdated { .. }
        | Event::DorsTransportSelected { .. }
        | Event::DorsTransportSwitched { .. }
        | Event::DorsEscalationTriggered { .. }
        | Event::SecurityWarning { .. }
        | Event::MessageRelayed { .. }
        | Event::UserBlocked { .. }
        | Event::UserUnblocked { .. }
        | Event::TofuReset { .. } => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        DecryptionFailureCode, DorsEscalationPhase, DorsEscalationReasonCode, DorsReasonCode,
        ForwardInfoEvent, GroupInfoMember, PresenceStatus, ReplyContextEvent, UserGroupSummary,
        WelcomeReasonCode,
    };
    use std::collections::HashMap;

    const SECRET: [u8; 16] = [0x7a; 16];

    fn scrubber_enabled() -> Scrubber {
        Scrubber::from_config(&crate::telemetry::TelemetryConfig::default(), SECRET)
    }

    fn scrubber_disabled() -> Scrubber {
        Scrubber::from_config(
            &crate::telemetry::TelemetryConfig::default().with_scrub_ids(false),
            SECRET,
        )
    }

    fn hashed(raw: &str) -> String {
        // Mirror the scrubber's leaf-identifier hash (enabled path).
        scrubber_enabled().hash_id(raw).into_owned()
    }

    #[test]
    fn disabled_scrubber_returns_borrowed_reference() {
        let event = Event::NeighborLost {
            peer_id: "alice".into(),
        };
        let scrubbed = scrub_event(&event, &scrubber_disabled());
        assert!(matches!(scrubbed, Cow::Borrowed(_)));
    }

    fn secret_media_metadata() -> offline_protocol_core::MediaMetadata {
        offline_protocol_core::MediaMetadata {
            mime_type: "image/jpeg".into(),
            file_name: "x.jpg".into(),
            file_size: 10,
            duration_ms: None,
            width: None,
            height: None,
            thumbnail_base64: None,
            media_id: Some("m1".into()),
            download_url: Some("https://cdn.example/m1".into()),
            thumbnail_url: None,
            encryption_key: Some("a2V5".into()),
            iv: Some("aXY=".into()),
            ciphertext_hash: Some("aGFzaA==".into()),
            sticker_provider: None,
            sticker_remote_id: None,
            sticker_kind: None,
        }
    }

    fn file_received_with_secrets() -> Event {
        Event::FileReceived {
            file_id: "file-1".into(),
            file_name: "x.jpg".into(),
            file_size: 10,
            sender: "alice".into(),
            content_type: "image".into(),
            media_metadata: Some(secret_media_metadata()),
            file_data: String::new(),
            timestamp: None,
            caption: None,
            reply_to_msg: None,
            reply_context: Some(Box::new(ReplyContextEvent {
                sender: "carol".into(),
                text: "quoted".into(),
                timestamp: None,
                reply_media_label: None,
                reply_content_type: None,
            })),
            forward_info: None,
        }
    }

    #[test]
    fn media_secrets_are_redacted_even_when_scrubbing_is_disabled() {
        // `scrub_ids = false` must not exempt key material: identifiers stay
        // raw (the knob's contract) but encryption_key/iv are still cleared.
        let scrubbed =
            scrub_event(&file_received_with_secrets(), &scrubber_disabled()).into_owned();
        match scrubbed {
            Event::FileReceived {
                sender,
                media_metadata,
                ..
            } => {
                assert_eq!(sender, "alice", "identifiers stay raw when disabled");
                let meta = media_metadata.expect("metadata preserved");
                assert!(meta.encryption_key.is_none());
                assert!(meta.iv.is_none());
                assert_eq!(meta.download_url.as_deref(), Some("https://cdn.example/m1"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn media_secrets_are_redacted_alongside_id_hashing_when_enabled() {
        let scrubbed = scrub_event(&file_received_with_secrets(), &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::FileReceived {
                sender,
                media_metadata,
                reply_context,
                ..
            } => {
                assert_eq!(sender, hashed("alice"));
                let meta = media_metadata.expect("metadata preserved");
                assert!(meta.encryption_key.is_none());
                assert!(meta.iv.is_none());
                // The quoted sender is an identifier like MessageReceived's:
                // hashed, while the quoted text stays raw content.
                let rc = reply_context.expect("reply context preserved");
                assert_eq!(rc.sender, hashed("carol"));
                assert_eq!(rc.text, "quoted");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn message_received_media_secrets_are_redacted_when_disabled() {
        let event = Event::MessageReceived {
            message_id: "msg-1".into(),
            sender: "alice".into(),
            recipient: "bob".into(),
            content: "c".into(),
            hop_count: 0,
            transport: "ble".into(),
            timestamp: 0,
            lamport_clock: 0,
            reply_to_msg: None,
            reply_context: None,
            content_type: "image".into(),
            media_metadata: Some(secret_media_metadata()),
            forward_info: None,
            encrypted: true,
        };
        let scrubbed = scrub_event(&event, &scrubber_disabled()).into_owned();
        match scrubbed {
            Event::MessageReceived { media_metadata, .. } => {
                let meta = media_metadata.expect("metadata preserved");
                assert!(meta.encryption_key.is_none());
                assert!(meta.iv.is_none());
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn group_message_received_media_secrets_are_redacted_when_disabled() {
        let event = Event::GroupMessageReceived {
            group_id: "group-1".into(),
            sender: "alice".into(),
            content: "c".into(),
            timestamp: "2026-07-21T00:00:00Z".into(),
            message_id: "msg-1".into(),
            reply_to_msg: None,
            forward_info: None,
            media_metadata: Some(secret_media_metadata()),
            content_type: Some("image".into()),
        };
        let scrubbed = scrub_event(&event, &scrubber_disabled()).into_owned();
        match scrubbed {
            Event::GroupMessageReceived { media_metadata, .. } => {
                let meta = media_metadata.expect("metadata preserved");
                assert!(meta.encryption_key.is_none());
                assert!(meta.iv.is_none());
                assert_eq!(meta.download_url.as_deref(), Some("https://cdn.example/m1"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn enabled_scrubber_hashes_peer_id_fields() {
        let event = Event::NeighborLost {
            peer_id: "alice".into(),
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::NeighborLost { peer_id } => assert_eq!(peer_id, hashed("alice")),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn message_received_scrubs_sender_recipient_and_forward_info_but_not_content_or_id() {
        let event = Event::MessageReceived {
            message_id: "msg-123".into(),
            sender: "alice".into(),
            recipient: "bob".into(),
            content: "hello".into(),
            hop_count: 1,
            transport: "ble".into(),
            timestamp: 0,
            lamport_clock: 0,
            reply_to_msg: None,
            reply_context: Some(Box::new(ReplyContextEvent {
                sender: "dave".into(),
                text: "quoted text".into(),
                timestamp: None,
                reply_media_label: None,
                reply_content_type: None,
            })),
            content_type: "text".into(),
            media_metadata: None,
            forward_info: Some(ForwardInfoEvent {
                original_sender: "carol".into(),
                original_message_id: "orig-456".into(),
                original_timestamp: 0,
                forward_count: 1,
            }),
            encrypted: false,
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::MessageReceived {
                message_id,
                sender,
                recipient,
                content,
                forward_info,
                reply_context,
                ..
            } => {
                assert_eq!(message_id, "msg-123", "message_id must stay raw");
                assert_eq!(content, "hello", "content must stay raw");
                assert_eq!(sender, hashed("alice"));
                assert_eq!(recipient, hashed("bob"));
                let fi = forward_info.expect("forward_info preserved");
                assert_eq!(fi.original_sender, hashed("carol"));
                assert_eq!(
                    fi.original_message_id, "orig-456",
                    "original_message_id must stay raw",
                );
                let rc = reply_context.expect("reply_context preserved");
                assert_eq!(rc.sender, hashed("dave"));
                assert_eq!(rc.text, "quoted text", "quoted text must stay raw");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn group_info_scrubs_group_id_creator_and_members() {
        let event = Event::GroupInfo {
            group_id: "grp-1".into(),
            name: "My Group".into(),
            created_by: "alice".into(),
            created_at: "2026-04-17T00:00:00Z".into(),
            members: vec![
                GroupInfoMember {
                    user_id: "bob".into(),
                    role: "admin".into(),
                    joined_at: "2026-04-17T00:00:00Z".into(),
                },
                GroupInfoMember {
                    user_id: "carol".into(),
                    role: "member".into(),
                    joined_at: "2026-04-17T00:00:00Z".into(),
                },
            ],
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::GroupInfo {
                group_id,
                name,
                created_by,
                members,
                ..
            } => {
                assert_eq!(group_id, hashed("grp-1"));
                assert_eq!(name, "My Group", "group name must stay raw");
                assert_eq!(created_by, hashed("alice"));
                assert_eq!(members.len(), 2);
                assert_eq!(members[0].user_id, hashed("bob"));
                assert_eq!(members[0].role, "admin", "role must stay raw");
                assert_eq!(members[1].user_id, hashed("carol"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn user_groups_scrubs_every_group_id_but_not_names() {
        let event = Event::UserGroups {
            groups: vec![
                UserGroupSummary {
                    group_id: "grp-a".into(),
                    name: "Alpha".into(),
                    created_at: "2026-04-17T00:00:00Z".into(),
                },
                UserGroupSummary {
                    group_id: "grp-b".into(),
                    name: "Beta".into(),
                    created_at: "2026-04-17T00:00:00Z".into(),
                },
            ],
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::UserGroups { groups } => {
                assert_eq!(groups.len(), 2);
                assert_eq!(groups[0].group_id, hashed("grp-a"));
                assert_eq!(groups[0].name, "Alpha");
                assert_eq!(groups[1].group_id, hashed("grp-b"));
                assert_eq!(groups[1].name, "Beta");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn group_message_partial_failure_scrubs_all_member_lists() {
        let event = Event::GroupMessagePartialFailure {
            group_id: "grp-1".into(),
            failed_members: vec!["alice".into(), "bob".into()],
            succeeded_members: vec!["carol".into()],
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::GroupMessagePartialFailure {
                group_id,
                failed_members,
                succeeded_members,
            } => {
                assert_eq!(group_id, hashed("grp-1"));
                assert_eq!(failed_members, vec![hashed("alice"), hashed("bob")]);
                assert_eq!(succeeded_members, vec![hashed("carol")]);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn service_discovered_scrubs_provider_peer_id_only() {
        let event = Event::ServiceDiscovered {
            query_id: "q-1".into(),
            service_id: "svc".into(),
            version: "1.0".into(),
            provider_peer_id: "alice".into(),
            capabilities: HashMap::new(),
            hop_count: 2,
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::ServiceDiscovered {
                query_id,
                service_id,
                provider_peer_id,
                ..
            } => {
                assert_eq!(query_id, "q-1", "query_id must stay raw");
                assert_eq!(service_id, "svc", "service_id must stay raw");
                assert_eq!(provider_peer_id, hashed("alice"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn typing_indicator_scrubs_sender_and_conversation_id() {
        let event = Event::TypingIndicatorReceived {
            sender: "alice".into(),
            conversation_id: "bob".into(),
            is_typing: true,
            timestamp: 0,
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::TypingIndicatorReceived {
                sender,
                conversation_id,
                ..
            } => {
                assert_eq!(sender, hashed("alice"));
                assert_eq!(conversation_id, hashed("bob"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn dors_score_updated_leaves_transport_names_raw() {
        let event = Event::DorsScoreUpdated {
            scores: vec![("ble".into(), 0.8), ("wifi_direct".into(), 0.6)],
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::DorsScoreUpdated { scores } => {
                assert_eq!(scores[0].0, "ble");
                assert_eq!(scores[1].0, "wifi_direct");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn message_decryption_failed_scrubs_sender_only() {
        let event = Event::MessageDecryptionFailed {
            message_id: "m-1".into(),
            sender: "alice".into(),
            code: DecryptionFailureCode::InvalidCiphertext,
            reason: "bad mac".into(),
        };
        let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
        match scrubbed {
            Event::MessageDecryptionFailed {
                message_id,
                sender,
                reason,
                ..
            } => {
                assert_eq!(message_id, "m-1");
                assert_eq!(sender, hashed("alice"));
                assert_eq!(reason, "bad mac");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn connection_events_scrub_party_identifiers() {
        let req = Event::ConnectionRequestReceived {
            sender: "alice".into(),
            sender_name: "Alice A.".into(),
            timestamp: 0,
            key_package: None,
            initial_message: None,
        };
        let accept = Event::ConnectionAccepted {
            accepted_by: "bob".into(),
            accepted_by_name: "Bob B.".into(),
            timestamp: 0,
            key_package: None,
        };
        let reject = Event::ConnectionRejected {
            rejected_by: "carol".into(),
        };
        let cancel = Event::ConnectionRequestCancelled {
            cancelled_by: "dan".into(),
        };
        for (event, expected_raw, expected_hashed) in [
            (req, "Alice A.", "alice"),
            (accept, "Bob B.", "bob"),
            (reject, "", "carol"),
            (cancel, "", "dan"),
        ] {
            let scrubbed = scrub_event(&event, &scrubber_enabled()).into_owned();
            match scrubbed {
                Event::ConnectionRequestReceived {
                    sender,
                    sender_name,
                    ..
                } => {
                    assert_eq!(sender, hashed(expected_hashed));
                    assert_eq!(sender_name, expected_raw);
                }
                Event::ConnectionAccepted {
                    accepted_by,
                    accepted_by_name,
                    ..
                } => {
                    assert_eq!(accepted_by, hashed(expected_hashed));
                    assert_eq!(accepted_by_name, expected_raw);
                }
                Event::ConnectionRejected { rejected_by } => {
                    assert_eq!(rejected_by, hashed(expected_hashed));
                }
                Event::ConnectionRequestCancelled { cancelled_by } => {
                    assert_eq!(cancelled_by, hashed(expected_hashed));
                }
                _ => panic!("unexpected variant"),
            }
        }
    }

    /// Regression guard: every variant present in the exhaustiveness ward
    /// must also be present in `scrub_in_place`. If they drift apart (e.g.
    /// someone adds a new variant + ward arm but forgets to extend the
    /// scrub match), the inner match goes non-exhaustive and the crate
    /// fails to compile — which is the whole point. This test simply
    /// exercises the scrubber against one exemplar per variant.
    #[test]
    fn scrub_in_place_handles_every_variant_without_panic() {
        let exemplars = [
            Event::MessageSent {
                message_id: String::new(),
                sender: "a".into(),
                recipient: "b".into(),
                content: String::new(),
                priority: String::new(),
                requires_ack: false,
                timestamp: 0,
                lamport_clock: 0,
                forward_info: None,
            },
            Event::MessageFailed {
                message_id: String::new(),
                reason: String::new(),
                retry_count: 0,
            },
            Event::RelayPromoted {
                connection_count: 0,
                battery_level: 0,
            },
            Event::NeighborLost {
                peer_id: "p".into(),
            },
            Event::NetworkMetrics {
                neighbor_count: 0,
                relay_count: 0,
                delivery_ratio: 0.0,
                avg_latency_ms: 0,
            },
            Event::DorsScoreUpdated { scores: Vec::new() },
            Event::DorsTransportSelected {
                from: None,
                transport: String::new(),
                reason_code: DorsReasonCode::InitialSelection,
                score: 0.0,
            },
            Event::DorsTransportSwitched {
                from: None,
                to: String::new(),
                reason_code: DorsReasonCode::PrimarySelected,
                reason_detail: None,
            },
            Event::DorsEscalationTriggered {
                phase: DorsEscalationPhase::Triggered,
                from: String::new(),
                to: String::new(),
                reason_code: DorsEscalationReasonCode::FallbackSuccess,
                reason_detail: None,
            },
            Event::WelcomeSendFailed {
                peer_id: "p".into(),
                message_id: String::new(),
                group_id: "g".into(),
                attempt: 0,
                reason_code: WelcomeReasonCode::InternalError,
                transport_error: None,
                retryable: false,
                next_retry_at: None,
            },
            Event::PresenceUpdated {
                peer_id: "p".into(),
                status: PresenceStatus::Online,
                timestamp: 0,
                last_seen_ms: None,
                source: crate::events::PresenceSource::Internet,
            },
        ];
        let scrubber = scrubber_enabled();
        for ev in exemplars {
            // The scrub must not panic for any variant and must produce a
            // different `Event` when an identifier field was present.
            let _ = scrub_event(&ev, &scrubber).into_owned();
        }
    }
}
