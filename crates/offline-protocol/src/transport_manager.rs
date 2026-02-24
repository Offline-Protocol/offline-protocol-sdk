//! Transport management for multi-transport protocol.
//!
//! This module provides the TransportManager which manages multiple transports
//! and uses DORS (Dynamic Offline Relay Switch) to select the optimal transport
//! for each message.

use crate::constants::{HOP_COUNT_EMA_ALPHA, LATENCY_EMA_ALPHA, OBSERVED_STATS_COMPACT_THRESHOLD};
use crate::{Error, Result};
use offline_protocol_core::Message;
use offline_protocol_router::{DorsConfig, TransportSelector};
use offline_protocol_transport::{
    Error as TransportError, Transport, TransportMetrics, TransportStatus, TransportType,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, warn};

/// Manages multiple transports and handles transport selection.
pub struct TransportManager {
    /// Available transports mapped by type.
    transports: HashMap<TransportType, Arc<Mutex<Box<dyn Transport>>>>,

    /// Current active transport.
    current_transport: Option<TransportType>,

    /// Transport selector (DORS).
    selector: TransportSelector,

    /// Locally observed delivery outcomes used to enrich transport metrics.
    observations: HashMap<TransportType, ObservedStats>,
}

#[derive(Debug, Default, Clone)]
struct ObservedStats {
    success_count: u32,
    failure_count: u32,
    latency_ema: Option<f32>,
    hop_ema: Option<f32>,
    last_success_at: Option<Instant>,
}

impl ObservedStats {
    fn record_success(&mut self, latency_ms: u32, hop_count: u8) {
        self.success_count = self.success_count.saturating_add(1);
        self.latency_ema = Some(update_ema(
            self.latency_ema,
            latency_ms as f32,
            LATENCY_EMA_ALPHA,
        ));
        self.hop_ema = Some(update_ema(
            self.hop_ema,
            hop_count as f32,
            HOP_COUNT_EMA_ALPHA,
        ));
        self.last_success_at = Some(Instant::now());
        self.compact();
    }

    fn record_failure(&mut self) {
        self.failure_count = self.failure_count.saturating_add(1);
        self.compact();
    }

    fn compact(&mut self) {
        let total = self.success_count.saturating_add(self.failure_count);
        if total > OBSERVED_STATS_COMPACT_THRESHOLD {
            self.success_count /= 2;
            self.failure_count /= 2;
        }
    }

    fn delivery_ratio(&self) -> Option<f32> {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            None
        } else {
            Some(self.success_count as f32 / total as f32)
        }
    }

    fn drop_ratio(&self) -> Option<f32> {
        self.delivery_ratio()
            .map(|ratio| (1.0 - ratio).clamp(0.0, 1.0))
    }

    fn apply_to_metrics(&self, metrics: &mut TransportMetrics) {
        if self.success_count + self.failure_count > 0 {
            // Only override delivery counts when the transport itself has not
            // reported any.  Internet transport tracks wire-level outcomes via
            // the confirmation loop; overwriting those would discard real data.
            //
            // NOTE: This check is value-based, not transport-type-based.  If
            // BLE or WiFi Direct ever start reporting their own success/failure
            // counts from the native layer (the way Internet does via the
            // confirmation loop), the condition below will silently skip the
            // local observations for those transports too.  At that point,
            // consider switching to an explicit per-transport-type check.
            if metrics.success_count + metrics.failure_count == 0 {
                metrics.success_count = self.success_count;
                metrics.failure_count = self.failure_count;
                metrics.delivery_ratio = self.delivery_ratio();
                metrics.drop_rate = self.drop_ratio();
            }
        }

        if let Some(latency) = self.latency_ema {
            metrics.latency_ms = Some(latency.round().clamp(0.0, u32::MAX as f32) as u32);
        }

        if let Some(hop) = self.hop_ema {
            metrics.average_hop_count = Some(hop);
        }
    }
}

fn update_ema(current: Option<f32>, new_value: f32, alpha: f32) -> f32 {
    let alpha = alpha.clamp(0.0, 1.0);
    match current {
        Some(existing) => existing * (1.0 - alpha) + new_value * alpha,
        None => new_value,
    }
}

impl TransportManager {
    /// Creates a new transport manager with default configuration.
    pub fn new(selector: TransportSelector) -> Self {
        Self {
            transports: HashMap::new(),
            current_transport: None,
            selector,
            observations: HashMap::new(),
        }
    }

    /// Adds a transport to the manager.
    ///
    /// # Arguments
    ///
    /// * `transport_type` - Type of transport to add
    /// * `transport` - The transport implementation
    pub fn add_transport(&mut self, transport_type: TransportType, transport: Box<dyn Transport>) {
        self.transports
            .insert(transport_type, Arc::new(Mutex::new(transport)));
    }

    /// Sends a message through the best available transport, with fallback.
    ///
    /// DORS selects a primary transport (applying hysteresis, cooldown, and
    /// stability checks). If the primary's `send()` fails synchronously, the
    /// remaining transports are tried in descending score order before
    /// returning an error. Each synchronous failure is recorded via
    /// `record_retry_failure` so DORS can adjust future scoring.
    ///
    /// # Internet transport — fallback does NOT apply
    ///
    /// Internet `send()` enqueues the message and returns `Ok(())`
    /// immediately. The actual wire-level outcome is reported asynchronously
    /// via the `confirm_sent` / `report_send_failure` confirmation loop on
    /// `InternetTransport`. Because `send()` never fails synchronously, the
    /// fallback loop below will **not** trigger for Internet. If the
    /// WebSocket is silently broken (status still `Available` but sends
    /// fail), messages will be enqueued, detected as failures by the
    /// confirmation-loop timeout, and counted in DORS metrics — but they
    /// are **not** retried on an alternative transport by this method.
    /// Retry/re-routing for those messages is handled by the higher-level
    /// outbox retry mechanism.
    pub fn send(&mut self, message: &Message) -> Result<()> {
        let available = self.get_available_transports();
        if available.is_empty() {
            return Err(Error::Transport(TransportError::TransportNotAvailable(
                "No available transport".to_string(),
            )));
        }

        // DORS selects the primary transport (applies hysteresis/cooldown/stability).
        let primary = self
            .selector
            .select_transport(message, &available)
            .ok_or_else(|| {
                Error::Transport(TransportError::TransportNotAvailable(
                    "No suitable transport selected from available transports".to_string(),
                ))
            })?;

        // Try the primary transport first.
        let primary_result = {
            let transport = self
                .transports
                .get(&primary)
                .ok_or_else(|| Error::Other(format!("Transport {:?} not found", primary)))?;
            let transport_lock = transport
                .lock()
                .map_err(|_| Error::Other(format!("Transport mutex poisoned for {:?}", primary)))?;
            transport_lock.send(message)
        };

        match primary_result {
            Ok(()) => {
                self.current_transport = Some(primary);
                return Ok(());
            }
            Err(e) => {
                warn!(
                    transport = ?primary,
                    error = %e,
                    "Primary transport send failed, trying fallback"
                );
                self.selector.record_retry_failure(primary);
            }
        }

        // Primary failed — try remaining transports in score order.
        // score_and_rank re-evaluates without hysteresis so every candidate
        // is considered. The re-scoring on the failure path is harmless.
        let scored = self.selector.score_and_rank(message, &available);
        let mut last_error = None;
        let mut attempted: Vec<TransportType> = vec![primary];

        for (transport_type, _score) in &scored {
            if *transport_type == primary {
                continue;
            }

            let transport = match self.transports.get(transport_type) {
                Some(t) => t,
                None => continue,
            };

            let transport_lock = match transport.lock() {
                Ok(lock) => lock,
                Err(_) => {
                    warn!(transport = ?transport_type, "Transport mutex poisoned, skipping");
                    continue;
                }
            };

            attempted.push(*transport_type);

            match transport_lock.send(message) {
                Ok(()) => {
                    self.current_transport = Some(*transport_type);
                    self.selector.set_current_transport(*transport_type);
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        transport = ?transport_type,
                        error = %e,
                        "Fallback transport send failed, trying next"
                    );
                    self.selector.record_retry_failure(*transport_type);
                    last_error = Some(e);
                }
            }
        }

        let terminal_error = last_error.unwrap_or_else(|| {
            TransportError::SendFailed(format!(
                "All transports failed (tried {:?})",
                attempted
            ))
        });
        Err(Error::Transport(terminal_error))
    }

    /// Sends a message via a specific transport, bypassing DORS selection.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    /// * `transport_type` - The transport to use
    ///
    /// # Returns
    ///
    /// Returns Ok(()) if sent successfully, Err otherwise.
    pub fn send_via_transport(
        &mut self,
        message: &Message,
        transport_type: TransportType,
    ) -> Result<()> {
        let transport = self
            .transports
            .get(&transport_type)
            .ok_or_else(|| Error::Other(format!("Transport {:?} not found", transport_type)))?;

        let transport_lock = transport.lock().map_err(|_| {
            Error::Other(format!("Transport mutex poisoned for {:?}", transport_type))
        })?;

        if transport_lock.status() != TransportStatus::Available {
            return Err(Error::Other(format!(
                "Transport {:?} is not available",
                transport_type
            )));
        }

        transport_lock
            .send(message)
            .map_err(|e| Error::Other(format!("Transport send failed: {}", e)))?;

        // Only update current transport after successful send
        self.current_transport = Some(transport_type);

        Ok(())
    }

    /// Attempts to receive a message from any transport.
    ///
    /// # Returns
    ///
    /// Returns Ok(Some((TransportType, Message))) if a message was received, Ok(None) if no message available.
    pub fn receive(&self) -> Result<Option<(TransportType, Message)>> {
        // Check all transports for messages
        for (transport_type, transport) in &self.transports {
            let transport_lock = match transport.lock() {
                Ok(lock) => lock,
                Err(_) => {
                    warn!(transport = ?transport_type, "Transport mutex poisoned, skipping");
                    continue;
                }
            };
            match transport_lock.receive() {
                Ok(Some(message)) => {
                    debug!(
                        transport = ?transport_type,
                        sender = %message.sender,
                        recipient = %message.recipient,
                        "TransportManager found message"
                    );
                    return Ok(Some((*transport_type, message)));
                }
                Ok(None) => {
                    // No message from this transport, continue checking others
                }
                Err(e) => {
                    warn!(
                        transport = ?transport_type,
                        error = %e,
                        "Transport receive error"
                    );
                }
            }
        }
        Ok(None)
    }

    /// Gets metrics for all available transports.
    ///
    /// # Returns
    ///
    /// Returns a map of transport type to metrics.
    pub fn get_available_transports(&self) -> HashMap<TransportType, TransportMetrics> {
        let mut available = HashMap::new();

        for (transport_type, transport) in &self.transports {
            let maybe_metrics = {
                let transport_lock = match transport.lock() {
                    Ok(lock) => lock,
                    Err(_) => {
                        warn!(transport = ?transport_type, "Transport mutex poisoned, skipping metrics");
                        continue;
                    }
                };
                let status = transport_lock.status();
                if status == TransportStatus::Available {
                    Some(transport_lock.metrics())
                } else {
                    debug!(
                        transport = ?transport_type,
                        status = ?status,
                        "Transport excluded from available set"
                    );
                    None
                }
            };

            if let Some(mut metrics) = maybe_metrics {
                if let Some(stats) = self.observations.get(transport_type) {
                    stats.apply_to_metrics(&mut metrics);
                }
                available.insert(*transport_type, metrics);
            }
        }

        available
    }

    /// Gets the current active transport type.
    pub fn current_transport(&self) -> Option<TransportType> {
        self.current_transport
    }

    /// Starts all transports.
    pub fn start(&mut self) -> Result<()> {
        for (transport_type, transport) in &self.transports {
            let mut transport_lock = transport.lock().map_err(|_| {
                Error::Other(format!("Transport mutex poisoned for {:?}", transport_type))
            })?;
            transport_lock.start().map_err(|e| {
                Error::Other(format!(
                    "Failed to start transport {:?}: {}",
                    transport_type, e
                ))
            })?;
        }
        Ok(())
    }

    /// Stops all transports.
    pub fn stop(&mut self) -> Result<()> {
        for (transport_type, transport) in &self.transports {
            let mut transport_lock = transport.lock().map_err(|_| {
                Error::Other(format!("Transport mutex poisoned for {:?}", transport_type))
            })?;
            transport_lock.stop().map_err(|e| {
                Error::Other(format!(
                    "Failed to stop transport {:?}: {}",
                    transport_type, e
                ))
            })?;
        }
        Ok(())
    }

    /// Gets a reference to a specific transport.
    pub fn get_transport(
        &self,
        transport_type: TransportType,
    ) -> Option<Arc<Mutex<Box<dyn Transport>>>> {
        self.transports.get(&transport_type).cloned()
    }

    /// Removes a transport from the manager.
    ///
    /// # Arguments
    ///
    /// * `transport_type` - Type of transport to remove
    pub fn remove_transport(&mut self, transport_type: TransportType) {
        self.transports.remove(&transport_type);

        // Clear current transport if it was the one removed
        if self.current_transport == Some(transport_type) {
            self.current_transport = None;
        }

        self.observations.remove(&transport_type);
    }

    /// Gets a list of all active transport types.
    ///
    /// # Returns
    ///
    /// Returns a vector of transport types that are currently added to the manager.
    pub fn get_active_transports(&self) -> Vec<TransportType> {
        self.transports.keys().copied().collect()
    }

    /// Checks if DORS suggests escalating from BLE to WiFi Direct.
    pub fn should_escalate_to_wifi(&self) -> bool {
        self.selector.should_escalate_to_wifi()
    }

    /// Records a retry failure for the given transport.
    pub fn record_retry_failure(&mut self, transport_type: TransportType) {
        self.selector.record_retry_failure(transport_type);
    }

    /// Resets retry count for the given transport after successful delivery.
    pub fn reset_retry_count(&mut self, transport_type: TransportType) {
        self.selector.reset_retry_count(transport_type);
    }

    /// Records a successful end-to-end delivery for the given transport.
    pub fn record_delivery_success(
        &mut self,
        transport_type: TransportType,
        latency_ms: u32,
        hop_count: u8,
    ) {
        let stats = self.observations.entry(transport_type).or_default();
        stats.record_success(latency_ms, hop_count);
    }

    /// Records a delivery failure (after exhausting retries) for the given transport.
    pub fn record_delivery_failure(&mut self, transport_type: TransportType) {
        let stats = self.observations.entry(transport_type).or_default();
        stats.record_failure();
    }

    /// Updates the DORS selector configuration at runtime.
    ///
    /// This replaces the current selector with a new one using the provided config.
    pub fn update_selector_config(&mut self, config: DorsConfig) {
        self.selector = TransportSelector::with_config(config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};
    use offline_protocol_router::DorsConfig;
    use offline_protocol_transport::{mock::MockTransport, TransportType};

    fn create_test_message() -> Message {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "Test message",
        )
    }

    #[test]
    fn test_transport_manager_creation() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let manager = TransportManager::new(selector);
        assert_eq!(manager.transports.len(), 0);
        assert!(manager.current_transport().is_none());
    }

    #[test]
    fn test_add_transport() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let transport = Box::new(MockTransport::new(TransportType::BLE));
        manager.add_transport(TransportType::BLE, transport);

        assert_eq!(manager.transports.len(), 1);
    }

    #[test]
    fn test_send_message() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        let message = create_test_message();
        let result = manager.send(&message);
        assert!(result.is_ok());
    }

    #[test]
    fn test_receive_message() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        let message = create_test_message();
        transport.queue_message(message.clone());

        manager.add_transport(TransportType::BLE, Box::new(transport));

        let received = manager.receive().unwrap();
        assert!(matches!(received, Some((TransportType::BLE, _))));
    }

    #[test]
    fn test_record_delivery_success_enriches_metrics() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        manager.record_delivery_success(TransportType::BLE, 150, 2);

        let metrics = manager.get_available_transports();
        let ble_metrics = metrics.get(&TransportType::BLE).expect("metrics");

        assert_eq!(ble_metrics.success_count, 1);
        assert_eq!(ble_metrics.failure_count, 0);
        assert!(ble_metrics.delivery_ratio.expect("delivery ratio") > 0.99);
        assert_eq!(ble_metrics.average_hop_count, Some(2.0));
        assert_eq!(ble_metrics.latency_ms, Some(150));
    }

    #[test]
    fn test_record_delivery_failure_enriches_metrics() {
        let selector = TransportSelector::with_config(DorsConfig::default());
        let mut manager = TransportManager::new(selector);

        let mut transport = MockTransport::new(TransportType::BLE);
        transport.start().unwrap();
        manager.add_transport(TransportType::BLE, Box::new(transport));

        manager.record_delivery_failure(TransportType::BLE);
        let metrics = manager.get_available_transports();
        let ble_metrics = metrics.get(&TransportType::BLE).expect("metrics");

        assert_eq!(ble_metrics.success_count, 0);
        assert_eq!(ble_metrics.failure_count, 1);
        assert!(ble_metrics.drop_rate.expect("drop ratio") > 0.99);
    }
}
