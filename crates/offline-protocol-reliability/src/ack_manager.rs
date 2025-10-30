//! ACK (Acknowledgment) manager for tracking message delivery

use offline_protocol_core::MessageId;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{debug, warn};

/// Acknowledgment information
#[derive(Debug, Clone)]
pub struct AckInfo {
    pub message_id: MessageId,
    pub hop_count: u8,
    pub latency_ms: u64,
}

/// Pending message awaiting ACK
struct PendingMessage {
    message_id: MessageId,
    sent_at: Instant,
    timeout: Duration,
    ack_tx: Option<oneshot::Sender<AckInfo>>,
}

/// Configuration for ACK manager
#[derive(Debug, Clone)]
pub struct AckManagerConfig {
    pub ack_timeout: Duration,
}

impl Default for AckManagerConfig {
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_secs(10),
        }
    }
}

/// Manages acknowledgments for sent messages
pub struct AckManager {
    config: AckManagerConfig,
    pending: Arc<RwLock<HashMap<MessageId, PendingMessage>>>,
}

impl AckManager {
    pub fn new(config: AckManagerConfig) -> Self {
        let manager = Self {
            config,
            pending: Arc::new(RwLock::new(HashMap::new())),
        };

        // Start timeout checker
        manager.start_timeout_checker();

        manager
    }

    /// Register a message awaiting ACK
    pub fn register(&self, message_id: MessageId) -> oneshot::Receiver<AckInfo> {
        let (ack_tx, ack_rx) = oneshot::channel();

        let pending_msg = PendingMessage {
            message_id,
            sent_at: Instant::now(),
            timeout: self.config.ack_timeout,
            ack_tx: Some(ack_tx),
        };

        self.pending.write().insert(message_id, pending_msg);
        debug!("Registered message {} for ACK", message_id);

        ack_rx
    }

    /// Handle received ACK
    pub fn handle_ack(&self, message_id: MessageId, hop_count: u8) {
        let mut pending = self.pending.write();
        
        if let Some(mut msg) = pending.remove(&message_id) {
            let latency_ms = msg.sent_at.elapsed().as_millis() as u64;
            
            debug!(
                "Received ACK for message {} (hops: {}, latency: {}ms)",
                message_id, hop_count, latency_ms
            );

            if let Some(tx) = msg.ack_tx.take() {
                let _ = tx.send(AckInfo {
                    message_id,
                    hop_count,
                    latency_ms,
                });
            }
        }
    }

    /// Check if a message is pending ACK
    pub fn is_pending(&self, message_id: MessageId) -> bool {
        self.pending.read().contains_key(&message_id)
    }

    /// Get the number of pending messages
    pub fn pending_count(&self) -> usize {
        self.pending.read().len()
    }

    /// Start background task to check for timeouts
    fn start_timeout_checker(&self) {
        let pending = Arc::clone(&self.pending);
        let timeout = self.config.ack_timeout;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));

            loop {
                interval.tick().await;

                let mut pending = pending.write();
                let now = Instant::now();
                let mut timed_out = Vec::new();

                for (message_id, msg) in pending.iter() {
                    if now.duration_since(msg.sent_at) > timeout {
                        timed_out.push(*message_id);
                    }
                }

                for message_id in timed_out {
                    if let Some(msg) = pending.remove(&message_id) {
                        warn!("ACK timeout for message {}", message_id);
                        // Dropping the oneshot sender will signal timeout to receiver
                        drop(msg.ack_tx);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ack_received() {
        let manager = AckManager::new(AckManagerConfig::default());
        let message_id = MessageId::new();

        let ack_rx = manager.register(message_id);
        
        // Simulate ACK receipt
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            manager.handle_ack(message_id, 3);
        });

        let ack_info = ack_rx.await.unwrap();
        assert_eq!(ack_info.message_id, message_id);
        assert_eq!(ack_info.hop_count, 3);
    }

    #[tokio::test]
    async fn test_ack_timeout() {
        let config = AckManagerConfig {
            ack_timeout: Duration::from_millis(100),
        };
        let manager = AckManager::new(config);
        let message_id = MessageId::new();

        let ack_rx = manager.register(message_id);

        // Don't send ACK, wait for timeout
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Should receive error due to timeout
        assert!(ack_rx.await.is_err());
    }
}

