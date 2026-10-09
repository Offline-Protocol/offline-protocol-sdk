//! Service request and response bodies sealed inside the 1:1 MLS session.
//!
//! `__SVC_REQ__` and `__SVC_RESP__` were signed plaintext before this existed,
//! readable by every device that carried them. They are now sealed toward a
//! peer that routes the sealed form (it advertised `svc_versions` entry 1 in a
//! signed key package, or sent us a sealed service frame) once the session with
//! that peer is confirmed. Toward everyone else the signed plaintext form is
//! still sent, because it is the floor every older peer depends on.
//!
//! Discovery (`__SVC_DISC_Q__` / `__SVC_DISC_R__`) never comes through here
//! sealed: it is a gossip every hop must read to match and forward, and it has
//! no single recipient whose session could seal it.

use super::{OfflineProtocol, OutboxReseal, MAX_KEY_PACKAGE_SENT_TO};
use crate::{Error, Result};
use offline_protocol_core::{ContentType, MessageId, MessagePriority};
use offline_protocol_services::{SVC_REQUEST, SVC_RESPONSE};
use tracing::{debug, warn};

impl OfflineProtocol {
    /// Sends a service frame to `recipient`, sealed when it can be and signed
    /// plaintext otherwise. Every service send goes through here, including
    /// the replies the service layer builds on its own (the automatic
    /// `not_found`), so a sealed request is never answered in the clear.
    ///
    /// The fallback toward a capable peer without a confirmed session is a
    /// stated exception to the capability rule that a downgrade loses the
    /// feature rather than the confidentiality: dropping the request instead
    /// would break the service for a window the application cannot see. The
    /// encrypt attempt is also what starts that session, exactly as it does
    /// for a direct message, so the next request seals.
    pub(crate) fn send_service_frame(
        &mut self,
        recipient: &str,
        content: String,
        priority: MessagePriority,
    ) -> Result<MessageId> {
        if !self.should_seal_service_frame(recipient, &content) {
            return self.send_internal_message(recipient, content, priority);
        }

        match self.encrypt_content_for_recipient(recipient, &content, priority) {
            Ok(sealed) => {
                let reseal = OutboxReseal {
                    content,
                    priority,
                    reply_to_msg: None,
                    forwarded_from: None,
                    content_type: ContentType::Text,
                    media_metadata: None,
                    rich: None,
                };
                self.send_sealed_internal_message(recipient, sealed, priority, Some(reseal))
            }
            Err(Error::SessionNotReady(_)) => {
                debug!(
                    recipient = %recipient,
                    "Session with a sealed-service peer not confirmed yet; service frame sent signed"
                );
                self.send_internal_message(recipient, content, priority)
            }
            Err(err) => {
                // A confirmed session that then fails to encrypt is the stale
                // confirmed-cache case. Losing the request is worse than the
                // exposure this fallback already accepts for an unconfirmed one.
                warn!(
                    recipient = %recipient,
                    error = %err,
                    "Sealing a service frame failed; sent signed instead"
                );
                self.send_internal_message(recipient, content, priority)
            }
        }
    }

    /// Whether `content` addressed to `recipient` should be attempted sealed:
    /// a request or response (never discovery), our switch on, MLS available,
    /// and a peer known to route the sealed form.
    fn should_seal_service_frame(&self, recipient: &str, content: &str) -> bool {
        (content.starts_with(SVC_REQUEST) || content.starts_with(SVC_RESPONSE))
            && self.config.encryption.encrypt_service_messages
            && self.should_auto_encrypt()
            && self.peer_svc_sealed.contains(recipient)
    }

    /// Records that `peer` routes sealed service frames, bounded like
    /// `key_package_sent_to`: the set is keyed by a sender id, and forgetting a
    /// peer costs only signed plaintext toward them until the next signal.
    pub(super) fn record_svc_sealed_peer(&mut self, peer: &str) {
        if !self.peer_svc_sealed.contains(peer)
            && self.peer_svc_sealed.len() >= MAX_KEY_PACKAGE_SENT_TO
        {
            self.peer_svc_sealed.clear();
        }
        self.peer_svc_sealed.insert(peer.to_string());
    }
}
