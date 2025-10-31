//! WebAssembly bindings for the Offline Protocol SDK.
//!
//! This crate provides WASM bindings for use in web browsers.
//! Note: Web browsers only support Internet transport (no BLE/Wi-Fi Direct).

use offline_protocol::{MessagePriority as CorePriority, OfflineProtocol as CoreProtocol, ProtocolConfig as CoreConfig};
use wasm_bindgen::prelude::*;

/// Sets panic hook for better error messages in the browser console.
pub fn set_panic_hook() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Message priority for WASM API.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub enum MessagePriority {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

impl From<MessagePriority> for CorePriority {
    fn from(priority: MessagePriority) -> Self {
        match priority {
            MessagePriority::Low => CorePriority::Low,
            MessagePriority::Medium => CorePriority::Medium,
            MessagePriority::High => CorePriority::High,
            MessagePriority::Critical => CorePriority::Critical,
        }
    }
}

/// Offline Protocol for WebAssembly.
#[wasm_bindgen]
pub struct OfflineProtocol {
    inner: CoreProtocol,
}

#[wasm_bindgen]
impl OfflineProtocol {
    /// Creates a new protocol instance from JSON configuration.
    ///
    /// # Example
    ///
    /// ```javascript
    /// const config = {
    ///   appId: 'my-web-app',
    ///   userId: 'user123',
    ///   transport: {
    ///     bleEnabled: false,        // Not available in browsers
    ///     wifiDirectEnabled: false, // Not available in browsers
    ///     internetEnabled: true,    // Only Internet works in web
    ///   }
    /// };
    /// 
    /// const protocol = new OfflineProtocol(JSON.stringify(config));
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn new(config_json: &str) -> Result<OfflineProtocol, JsValue> {
        set_panic_hook();

        // Parse configuration
        let config_value: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid JSON: {}", e)))?;

        let app_id = config_value["appId"]
            .as_str()
            .ok_or_else(|| JsValue::from_str("Missing appId"))?;
        let user_id = config_value["userId"]
            .as_str()
            .ok_or_else(|| JsValue::from_str("Missing userId"))?;

        let config = CoreConfig::new(app_id, user_id);

        let protocol = CoreProtocol::new(config)
            .map_err(|e| JsValue::from_str(&format!("Failed to create protocol: {}", e)))?;

        Ok(Self { inner: protocol })
    }

    /// Starts the protocol.
    pub fn start(&mut self) -> Result<(), JsValue> {
        self.inner
            .start()
            .map_err(|e| JsValue::from_str(&format!("Failed to start: {}", e)))
    }

    /// Stops the protocol.
    pub fn stop(&mut self) -> Result<(), JsValue> {
        self.inner
            .stop()
            .map_err(|e| JsValue::from_str(&format!("Failed to stop: {}", e)))
    }

    /// Sends a message.
    ///
    /// Returns the message ID as a string.
    #[wasm_bindgen(js_name = sendMessage)]
    pub fn send_message(
        &mut self,
        recipient: &str,
        content: &str,
        priority: MessagePriority,
    ) -> Result<String, JsValue> {
        let message_id = self
            .inner
            .send_message(recipient, content, Some(priority.into()))
            .map_err(|e| JsValue::from_str(&format!("Failed to send message: {}", e)))?;

        Ok(message_id.as_str())
    }

    /// Gets the current protocol state.
    #[wasm_bindgen(js_name = getState)]
    pub fn get_state(&self) -> String {
        format!("{:?}", self.inner.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn test_protocol_creation() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let protocol = OfflineProtocol::new(config);
        assert!(protocol.is_ok());
    }

    #[wasm_bindgen_test]
    fn test_invalid_config() {
        let config = r#"{"appId": "test-app"}"#; // Missing userId
        let protocol = OfflineProtocol::new(config);
        assert!(protocol.is_err());
    }
}

