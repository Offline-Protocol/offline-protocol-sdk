//! C FFI bindings for the Offline Protocol SDK.
//!
//! This crate exposes a C-compatible API for cross-platform interoperability.
//! This is the ONLY crate in the SDK that contains unsafe code, isolated to
//! FFI boundaries.
//!
//! # Safety
//!
//! All unsafe code in this crate is documented with SAFETY comments explaining
//! why the operations are safe. All pointers are validated before use, and
//! panics are caught to prevent unwinding across FFI boundaries.

#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![warn(missing_docs)]

use offline_protocol::{OfflineProtocol, ProtocolConfig, NetworkVisualizer};
use offline_protocol_router::DorsConfig;
use offline_protocol_transport::{BleTransport, PeerDevice, TransportStatus};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::panic;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Success code.
pub const SUCCESS: i32 = 0;

/// No BLE fragment available.
pub const NO_FRAGMENT_AVAILABLE: i32 = 1;

/// Error: Null pointer passed as argument.
pub const ERROR_NULL_POINTER: i32 = -1;

/// Error: Invalid UTF-8 in string parameter.
pub const ERROR_INVALID_UTF8: i32 = -2;

/// Error: Protocol not started.
pub const ERROR_NOT_STARTED: i32 = -3;

/// Error: Protocol already started.
pub const ERROR_ALREADY_STARTED: i32 = -4;

/// Error: Failed to send message.
pub const ERROR_SEND_FAILED: i32 = -5;

/// Error: Invalid configuration.
pub const ERROR_INVALID_CONFIG: i32 = -6;

/// Error: Rust panic occurred (bug).
pub const ERROR_PANIC: i32 = -99;

/// Error: Other unspecified error.
pub const ERROR_OTHER: i32 = -100;

/// Event callback function type.
///
/// # Arguments
///
/// * `event_json` - JSON string representing the event
/// * `user_data` - Opaque pointer to user data passed to set_event_callback
///
/// # Safety
///
/// The callback must be C ABI compatible and thread-safe.
pub type EventCallback = extern "C" fn(event_json: *const c_char, user_data: *mut c_void);

/// Thread-safe wrapper for callback data.
///
/// # Safety
///
/// The caller must ensure that:
/// - The callback function pointer is thread-safe
/// - The user_data pointer remains valid for the lifetime of the protocol
/// - The user_data pointer can be safely accessed from any thread
struct CallbackData {
    callback: EventCallback,
    user_data: *mut c_void,
}

// SAFETY: The caller of set_event_callback is responsible for ensuring
// that the callback function is thread-safe and that user_data can be
// safely accessed from any thread. This is documented in the function's
// safety contract.
unsafe impl Send for CallbackData {}
unsafe impl Sync for CallbackData {}

/// Wrapper for OfflineProtocol with event callback support.
///
/// This is an opaque type used only via pointers in the FFI interface.
pub struct ProtocolHandle {
    protocol: OfflineProtocol,
    callback: Arc<Mutex<Option<CallbackData>>>,
    ble_transport: Arc<Mutex<Option<Arc<BleTransport>>>>,
    visualizer: Arc<Mutex<NetworkVisualizer>>,
}

/// Creates a new OfflineProtocol instance from JSON configuration.
///
/// # Safety
///
/// `config_json` must be a valid null-terminated C string.
/// Returns a pointer to ProtocolHandle on success, or null on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_create(config_json: *const c_char) -> *mut ProtocolHandle {
    // Validate pointer
    if config_json.is_null() {
        return ptr::null_mut();
    }

    // Catch any panics
    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated config_json is non-null above.
        // The caller must ensure it's a valid C string.
        let config_str = match CStr::from_ptr(config_json).to_str() {
            Ok(s) => s,
            Err(_) => return ptr::null_mut(),
        };

        // Parse configuration (expecting JSON with app_id and user_id at minimum)
        let config: Result<serde_json::Value, _> = serde_json::from_str(config_str);
        let config_value = match config {
            Ok(v) => v,
            Err(_) => return ptr::null_mut(),
        };

        // Extract required fields
        let app_id = config_value["appId"]
            .as_str()
            .or_else(|| config_value["app_id"].as_str())
            .unwrap_or("");
        let user_id = config_value["userId"]
            .as_str()
            .or_else(|| config_value["user_id"].as_str())
            .unwrap_or("");

        if app_id.is_empty() || user_id.is_empty() {
            return ptr::null_mut();
        }

        // Create protocol config with DORS configuration
        let mut protocol_config = ProtocolConfig::new(app_id, user_id);
        
        // Parse DORS configuration if provided
        if let Some(dors_config) = config_value.get("dors") {
            let mut dors = DorsConfig::default();
            
            if let Some(prefer_online) = dors_config.get("preferOnline").and_then(|v| v.as_bool()) {
                dors.prefer_online = prefer_online;
            }
            if let Some(hysteresis) = dors_config.get("switchHysteresis").and_then(|v| v.as_f64()) {
                dors.switch_hysteresis = hysteresis as f32;
            }
            if let Some(cooldown) = dors_config.get("switchCooldownSecs").and_then(|v| v.as_u64()) {
                dors.switch_cooldown_secs = cooldown;
            }
            if let Some(threshold) = dors_config.get("bleToWifiRetryThreshold").and_then(|v| v.as_u64()) {
                dors.ble_to_wifi_retry_threshold = threshold as u32;
            }
            if let Some(rssi) = dors_config.get("rssiSwitchThreshold").and_then(|v| v.as_i64()) {
                dors.rssi_switch_threshold = rssi as i16;
            }
            if let Some(queue) = dors_config.get("congestionQueueThreshold").and_then(|v| v.as_u64()) {
                dors.congestion_queue_threshold = queue as usize;
            }
            if let Some(window) = dors_config.get("stabilityWindowSecs").and_then(|v| v.as_u64()) {
                dors.stability_window_secs = window;
            }
            
            protocol_config.dors = dors;
        }

        // Create protocol instance
        match OfflineProtocol::new(protocol_config) {
            Ok(mut protocol) => {
                use offline_protocol_transport::{Transport, TransportType, InternetTransport, InternetConfig, WifiDirectTransport, WifiDirectConfig};
                
                // Parse transports configuration
                let transports_config = config_value.get("transports");
                
                // BLE transport (always created for FFI, but may not be enabled)
                let ble_transport = Arc::new(BleTransport::new(user_id));
                let ble_enabled = transports_config
                    .and_then(|t| t.get("ble"))
                    .and_then(|b| b.get("enabled"))
                    .and_then(|e| e.as_bool())
                    .unwrap_or(true); // Default: enabled
                
                if ble_enabled {
                    // We need to convert Arc<BleTransport> to Box<dyn Transport>
                    // Create a wrapper that holds the Arc
                    struct ArcBleTransport(Arc<BleTransport>);
                    impl Transport for ArcBleTransport {
                        fn transport_type(&self) -> TransportType {
                            self.0.transport_type()
                        }
                        fn status(&self) -> TransportStatus {
                            self.0.status()
                        }
                        fn metrics(&self) -> offline_protocol_transport::TransportMetrics {
                            self.0.metrics()
                        }
                        fn send(&self, message: &offline_protocol_core::Message) -> offline_protocol_transport::Result<()> {
                            self.0.send(message)
                        }
                        fn receive(&self) -> offline_protocol_transport::Result<Option<offline_protocol_core::Message>> {
                            self.0.receive()
                        }
                        fn start(&mut self) -> offline_protocol_transport::Result<()> {
                            // Status will be updated via on_status_changed
                            Ok(())
                        }
                        fn stop(&mut self) -> offline_protocol_transport::Result<()> {
                            // Status will be updated via on_status_changed  
                            Ok(())
                        }
                    }
                    
                    let ble_clone = ble_transport.clone();
                    protocol.transport_manager_mut().add_transport(
                        TransportType::BLE,
                        Box::new(ArcBleTransport(ble_clone))
                    );
                }
                
                // Internet transport (optional)
                if let Some(internet_cfg) = transports_config
                    .and_then(|t| t.get("internet"))
                    .filter(|i| i.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false))
                {
                    let mut config = InternetConfig::default();
                    if let Some(addr) = internet_cfg.get("serverAddress").and_then(|v| v.as_str()) {
                        config.server_address = addr.to_string();
                    }
                    if let Some(auto_reconnect) = internet_cfg.get("autoReconnect").and_then(|v| v.as_bool()) {
                        config.auto_reconnect = auto_reconnect;
                    }
                    
                    let transport = InternetTransport::with_config(user_id, config);
                    protocol.transport_manager_mut().add_transport(
                        TransportType::Internet,
                        Box::new(transport)
                    );
                }
                
                // WiFi Direct transport (optional)
                if let Some(wifi_cfg) = transports_config
                    .and_then(|t| t.get("wifiDirect"))
                    .filter(|w| w.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false))
                {
                    let mut config = WifiDirectConfig::default();
                    if let Some(name) = wifi_cfg.get("deviceName").and_then(|v| v.as_str()) {
                        config.device_name = name.to_string();
                    }
                    if let Some(auto_accept) = wifi_cfg.get("autoAccept").and_then(|v| v.as_bool()) {
                        config.auto_accept = auto_accept;
                    }
                    if let Some(intent) = wifi_cfg.get("groupOwnerIntent").and_then(|v| v.as_u64()) {
                        config.group_owner_intent = intent as u8;
                    }
                    
                    let transport = WifiDirectTransport::with_config(user_id, config);
                    protocol.transport_manager_mut().add_transport(
                        TransportType::WiFiDirect,
                        Box::new(transport)
                    );
                }
                
                let visualizer = NetworkVisualizer::new(user_id);
                let handle = ProtocolHandle {
                    protocol,
                    callback: Arc::new(Mutex::new(None)),
                    ble_transport: Arc::new(Mutex::new(Some(ble_transport))),
                    visualizer: Arc::new(Mutex::new(visualizer)),
                };
                Box::into_raw(Box::new(handle))
            }
            Err(_) => ptr::null_mut(),
        }
    });

    result.unwrap_or(ptr::null_mut())
}

/// Destroys an OfflineProtocol instance and frees its memory.
///
/// # Safety
///
/// `handle` must be a valid pointer returned by `offline_protocol_create`.
/// After calling this function, the handle is invalid and must not be used.
#[no_mangle]
pub extern "C" fn offline_protocol_destroy(handle: *mut ProtocolHandle) {
    if handle.is_null() {
        return;
    }

    unsafe {
        // SAFETY: We validated handle is non-null. The caller must ensure
        // this is a valid ProtocolHandle pointer created by offline_protocol_create.
        let _ = Box::from_raw(handle);
        // Box is dropped here, freeing the memory
    }
}

/// Starts the protocol.
///
/// # Safety
///
/// `handle` must be a valid pointer returned by `offline_protocol_create`.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_start(handle: *mut ProtocolHandle) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null and it was created by
        // offline_protocol_create, so we know it's a valid ProtocolHandle.
        let handle_ref = &mut *handle;

        match handle_ref.protocol.start() {
            Ok(_) => SUCCESS,
            Err(offline_protocol::Error::AlreadyStarted) => ERROR_ALREADY_STARTED,
            Err(_) => ERROR_OTHER,
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Stops the protocol.
///
/// # Safety
///
/// `handle` must be a valid pointer returned by `offline_protocol_create`.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_stop(handle: *mut ProtocolHandle) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null and it was created by
        // offline_protocol_create, so we know it's a valid ProtocolHandle.
        let handle_ref = &mut *handle;

        match handle_ref.protocol.stop() {
            Ok(_) => SUCCESS,
            Err(offline_protocol::Error::NotStarted) => ERROR_NOT_STARTED,
            Err(_) => ERROR_OTHER,
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Sends a message.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `recipient` and `content` must be valid null-terminated C strings.
/// - `out_message_id` must point to a buffer of at least `out_len` bytes.
///
/// # Returns
///
/// Returns SUCCESS and writes message ID to `out_message_id`, or an error code.
#[no_mangle]
pub extern "C" fn offline_protocol_send_message(
    handle: *mut ProtocolHandle,
    recipient: *const c_char,
    content: *const c_char,
    priority: i32,
    out_message_id: *mut c_char,
    out_len: usize,
) -> i32 {
    // Validate pointers
    if handle.is_null() || recipient.is_null() || content.is_null() || out_message_id.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated all pointers are non-null above.
        // The caller must ensure they point to valid data.
        let handle_ref = &mut *handle;

        // Convert C strings to Rust strings
        let recipient_str = match CStr::from_ptr(recipient).to_str() {
            Ok(s) => s,
            Err(_) => return ERROR_INVALID_UTF8,
        };

        let content_str = match CStr::from_ptr(content).to_str() {
            Ok(s) => s,
            Err(_) => return ERROR_INVALID_UTF8,
        };

        // Map priority (0=Low, 1=Medium, 2=High, 3=Critical)
        let priority = match priority {
            0 => offline_protocol_core::MessagePriority::Low,
            1 => offline_protocol_core::MessagePriority::Medium,
            2 => offline_protocol_core::MessagePriority::High,
            3 => offline_protocol_core::MessagePriority::Critical,
            _ => offline_protocol_core::MessagePriority::Medium,
        };

        // Send message
        let message_id = match handle_ref
            .protocol
            .send_message(recipient_str, content_str, Some(priority))
        {
            Ok(id) => id,
            Err(offline_protocol::Error::NotStarted) => return ERROR_NOT_STARTED,
            Err(_) => return ERROR_SEND_FAILED,
        };

        // Copy message ID to output buffer
        let id_str = message_id.as_str();
        let id_bytes = id_str.as_bytes();
        
        if id_bytes.len() >= out_len {
            return ERROR_OTHER; // Buffer too small
        }

        ptr::copy_nonoverlapping(id_bytes.as_ptr(), out_message_id as *mut u8, id_bytes.len());
        // Null terminate
        *out_message_id.add(id_bytes.len()) = 0;

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Polls for the next event.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_event_json` must point to a buffer of at least `out_len` bytes.
///
/// # Returns
///
/// Returns SUCCESS if an event was retrieved, 0 if no event available, or an error code.
#[no_mangle]
pub extern "C" fn offline_protocol_poll_event(
    handle: *mut ProtocolHandle,
    out_event_json: *mut c_char,
    _out_len: usize,
) -> i32 {
    if handle.is_null() || out_event_json.is_null() {
        return ERROR_NULL_POINTER;
    }

    // For now, return 0 (no event) as event polling needs more infrastructure
    // This will be properly implemented when integrating with platform event loops
    0
}

/// Sets an event callback to receive protocol events.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `callback` must be a valid C function pointer with the EventCallback signature.
/// - `user_data` is an opaque pointer that will be passed back to the callback.
/// - The callback must be thread-safe as it may be invoked from any thread.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_set_event_callback(
    handle: *mut ProtocolHandle,
    callback: Option<EventCallback>,
    user_data: *mut c_void,
) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null and it was created by
        // offline_protocol_create, so we know it's a valid ProtocolHandle.
        let handle_ref = &mut *handle;

        // Store the callback
        let mut cb = handle_ref.callback.lock().unwrap();
        *cb = callback.map(|f| CallbackData {
            callback: f,
            user_data,
        });
        drop(cb);

        // Register the event handler with the protocol
        let callback_arc = handle_ref.callback.clone();
        handle_ref.protocol.on_event(move |event| {
            // Serialize event to JSON
            let event_json = match event.to_json() {
                Ok(json) => json,
                Err(_) => return, // Skip if serialization fails
            };

            // Convert to C string
            let c_str = match CString::new(event_json) {
                Ok(s) => s,
                Err(_) => return, // Skip if contains null bytes
            };

            // Call the callback if set
            let cb_guard = callback_arc.lock().unwrap();
            if let Some(ref cb_data) = *cb_guard {
                // SAFETY: The callback function pointer must be valid and the user_data
                // pointer must remain valid for the lifetime of the protocol.
                (cb_data.callback)(c_str.as_ptr(), cb_data.user_data);
            }
        });

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Frees a string allocated by the FFI layer.
///
/// # Safety
///
/// `s` must be a pointer returned by an FFI function that allocates strings.
#[no_mangle]
pub extern "C" fn offline_protocol_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }

    unsafe {
        // SAFETY: We validated s is non-null. The caller must ensure this
        // was allocated by CString::into_raw().
        let _ = CString::from_raw(s);
        // CString is dropped here, freeing the memory
    }
}

/// Notifies the BLE transport that a peer has been discovered.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `device_id` and `address` must be valid null-terminated C strings.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_peer_discovered(
    handle: *mut ProtocolHandle,
    device_id: *const c_char,
    address: *const c_char,
    rssi: i16,
) -> i32 {
    if handle.is_null() || device_id.is_null() || address.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated all pointers are non-null above.
        let handle_ref = &*handle;

        // Convert C strings to Rust strings
        let device_id_str = match CStr::from_ptr(device_id).to_str() {
            Ok(s) => s,
            Err(_) => return ERROR_INVALID_UTF8,
        };

        let address_str = match CStr::from_ptr(address).to_str() {
            Ok(s) => s,
            Err(_) => return ERROR_INVALID_UTF8,
        };

        // Get BLE transport
        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            // Create peer device
            let peer = PeerDevice {
                device_id: device_id_str.to_string(),
                address: address_str.to_string(),
                rssi,
                last_seen: SystemTime::now(),
                connected: true,
            };

            // Notify transport
            ble_transport.on_peer_discovered(peer);

            SUCCESS
        } else {
            ERROR_OTHER
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Notifies the BLE transport that a peer has been lost.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `device_id` must be a valid null-terminated C string.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_peer_lost(
    handle: *mut ProtocolHandle,
    device_id: *const c_char,
) -> i32 {
    if handle.is_null() || device_id.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated all pointers are non-null above.
        let handle_ref = &*handle;

        // Convert C string to Rust string
        let device_id_str = match CStr::from_ptr(device_id).to_str() {
            Ok(s) => s,
            Err(_) => return ERROR_INVALID_UTF8,
        };

        // Get BLE transport
        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            ble_transport.on_peer_lost(device_id_str);
            SUCCESS
        } else {
            ERROR_OTHER
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Notifies the BLE transport of a status change.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
///
/// # Arguments
///
/// - `status`: 0 = Unavailable, 1 = Available, 2 = Disconnected
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_status_changed(
    handle: *mut ProtocolHandle,
    status: i32,
) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null above.
        let handle_ref = &*handle;

        // Map status code to TransportStatus
        let transport_status = match status {
            0 => TransportStatus::Unavailable,
            1 => TransportStatus::Available,
            2 => TransportStatus::Disconnected,
            _ => TransportStatus::Unavailable,
        };

        // Get BLE transport
        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            ble_transport.on_status_changed(transport_status);
            SUCCESS
        } else {
            ERROR_OTHER
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Called when a BLE fragment is received from a peer.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `fragment_data` must be a valid pointer to a byte array of length `data_len`.
///
/// # Arguments
///
/// - `fragment_data`: Pointer to the fragment byte array
/// - `data_len`: Length of the fragment data
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_fragment_received(
    handle: *mut ProtocolHandle,
    fragment_data: *const u8,
    data_len: usize,
) -> i32 {
    if handle.is_null() || fragment_data.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated pointers are non-null above.
        let handle_ref = &*handle;

        // Copy fragment data
        let data_slice = std::slice::from_raw_parts(fragment_data, data_len);
        let data_vec = data_slice.to_vec();

        // Get BLE transport
        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            match ble_transport.on_fragment_received(data_vec) {
                Ok(()) => SUCCESS,
                Err(_) => ERROR_OTHER,
            }
        } else {
            ERROR_OTHER
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets the next BLE fragment to send.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `recipient_out` must be a valid pointer to a buffer of at least 256 bytes.
/// - `fragment_out` must be a valid pointer to a buffer of at least `fragment_out_len` bytes.
/// - `fragment_len_out` must be a valid pointer to store the actual fragment length.
///
/// # Arguments
///
/// - `recipient_out`: Buffer to write the recipient device ID (null-terminated)
/// - `recipient_out_len`: Size of the recipient buffer
/// - `fragment_out`: Buffer to write the fragment data
/// - `fragment_out_len`: Size of the fragment buffer
/// - `fragment_len_out`: Pointer to store the actual fragment length
///
/// # Returns
///
/// Returns SUCCESS if a fragment is available, NO_FRAGMENT_AVAILABLE if none are queued, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_get_next_fragment(
    handle: *mut ProtocolHandle,
    recipient_out: *mut c_char,
    recipient_out_len: usize,
    fragment_out: *mut u8,
    fragment_out_len: usize,
    fragment_len_out: *mut usize,
) -> i32 {
    if handle.is_null() || recipient_out.is_null() || fragment_out.is_null() || fragment_len_out.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated pointers are non-null above.
        let handle_ref = &*handle;

        *fragment_len_out = 0;
        if recipient_out_len > 0 {
            *recipient_out = 0;
        }

        // Get BLE transport
        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            match ble_transport.get_next_fragment() {
                Ok(Some((recipient, fragment_data))) => {
                    // Write recipient
                    let recipient_cstr = match CString::new(recipient) {
                        Ok(s) => s,
                        Err(_) => return ERROR_INVALID_UTF8,
                    };
                    let recipient_bytes = recipient_cstr.as_bytes_with_nul();
                    if recipient_bytes.len() > recipient_out_len {
                        return ERROR_OTHER;
                    }
                    ptr::copy_nonoverlapping(
                        recipient_bytes.as_ptr(),
                        recipient_out as *mut u8,
                        recipient_bytes.len(),
                    );

                    // Write fragment data
                    if fragment_data.len() > fragment_out_len {
                        return ERROR_OTHER;
                    }
                    ptr::copy_nonoverlapping(
                        fragment_data.as_ptr(),
                        fragment_out,
                        fragment_data.len(),
                    );
                    *fragment_len_out = fragment_data.len();

                    SUCCESS
                }
                Ok(None) => NO_FRAGMENT_AVAILABLE,
                Err(_) => ERROR_OTHER,
            }
        } else {
            ERROR_OTHER
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Re-queues a BLE fragment if sending fails on the platform side.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_return_fragment(
    handle: *mut ProtocolHandle,
    recipient: *const c_char,
    fragment_data: *const u8,
    fragment_len: usize,
) -> i32 {
    if handle.is_null() || recipient.is_null() || fragment_data.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        let handle_ref = &*handle;

        let recipient_str = match CStr::from_ptr(recipient).to_str() {
            Ok(s) => s,
            Err(_) => return ERROR_INVALID_UTF8,
        };

        let data_slice = std::slice::from_raw_parts(fragment_data, fragment_len);
        let mut fragment_vec = Vec::with_capacity(fragment_len);
        fragment_vec.extend_from_slice(data_slice);

        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            ble_transport.requeue_fragment(recipient_str, fragment_vec);
            SUCCESS
        } else {
            ERROR_OTHER
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets the number of discovered peers.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
///
/// # Returns
///
/// Returns the number of discovered peers, or -1 on error.
#[no_mangle]
pub extern "C" fn offline_protocol_ble_get_peer_count(
    handle: *mut ProtocolHandle,
) -> i32 {
    if handle.is_null() {
        return -1;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null above.
        let handle_ref = &*handle;

        // Get BLE transport
        let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
        if let Some(ref ble_transport) = *ble_transport_opt {
            let peers = ble_transport.get_peers();
            peers.len() as i32
        } else {
            -1
        }
    });

    result.unwrap_or(-1)
}

/// Adds an Internet transport to the protocol.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `config_json` must be a valid null-terminated C string containing JSON configuration.
///
/// Configuration JSON format:
/// ```json
/// {
///   "serverAddress": "wss://relay.example.com",
///   "autoReconnect": true,
///   "reconnectDelay": 5000
/// }
/// ```
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_add_internet_transport(
    handle: *mut ProtocolHandle,
    config_json: *const c_char,
) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null above.
        let handle_ref = &mut *(handle as *mut ProtocolHandle);

        // Parse config if provided
        let config = if config_json.is_null() {
            offline_protocol_transport::InternetConfig::default()
        } else {
            let config_str = match CStr::from_ptr(config_json).to_str() {
                Ok(s) => s,
                Err(_) => return ERROR_INVALID_UTF8,
            };

            let config_value: serde_json::Value = match serde_json::from_str(config_str) {
                Ok(v) => v,
                Err(_) => return ERROR_INVALID_CONFIG,
            };

            let mut config = offline_protocol_transport::InternetConfig::default();
            
            if let Some(addr) = config_value["serverAddress"].as_str() {
                config.server_address = addr.to_string();
            }
            if let Some(auto_reconnect) = config_value["autoReconnect"].as_bool() {
                config.auto_reconnect = auto_reconnect;
            }
            if let Some(delay_ms) = config_value["reconnectDelay"].as_u64() {
                config.reconnect_delay = std::time::Duration::from_millis(delay_ms);
            }

            config
        };

        // Get user_id from protocol config
        let user_id = handle_ref.protocol.config().user_id.as_str();
        
        // Create Internet transport
        let transport = offline_protocol_transport::InternetTransport::with_config(
            user_id,
            config
        );

        // Add to protocol's transport manager
        use offline_protocol_transport::TransportType;
        handle_ref.protocol.transport_manager_mut().add_transport(
            TransportType::Internet,
            Box::new(transport)
        );

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Adds a WiFi Direct transport to the protocol.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `config_json` must be a valid null-terminated C string containing JSON configuration.
///
/// Configuration JSON format:
/// ```json
/// {
///   "deviceName": "MyDevice",
///   "autoAccept": false,
///   "groupOwnerIntent": 7
/// }
/// ```
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_add_wifi_direct_transport(
    handle: *mut ProtocolHandle,
    config_json: *const c_char,
) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null above.
        let handle_ref = &mut *(handle as *mut ProtocolHandle);

        // Parse config if provided
        let config = if config_json.is_null() {
            offline_protocol_transport::WifiDirectConfig::default()
        } else {
            let config_str = match CStr::from_ptr(config_json).to_str() {
                Ok(s) => s,
                Err(_) => return ERROR_INVALID_UTF8,
            };

            let config_value: serde_json::Value = match serde_json::from_str(config_str) {
                Ok(v) => v,
                Err(_) => return ERROR_INVALID_CONFIG,
            };

            let mut config = offline_protocol_transport::WifiDirectConfig::default();
            
            if let Some(name) = config_value["deviceName"].as_str() {
                config.device_name = name.to_string();
            }
            if let Some(auto_accept) = config_value["autoAccept"].as_bool() {
                config.auto_accept = auto_accept;
            }
            if let Some(intent) = config_value["groupOwnerIntent"].as_u64() {
                config.group_owner_intent = intent as u8;
            }

            config
        };

        // Get user_id from protocol config
        let user_id = handle_ref.protocol.config().user_id.as_str();
        
        // Create WiFi Direct transport
        let transport = offline_protocol_transport::WifiDirectTransport::with_config(
            user_id,
            config
        );

        // Add to protocol's transport manager
        use offline_protocol_transport::TransportType;
        handle_ref.protocol.transport_manager_mut().add_transport(
            TransportType::WiFiDirect,
            Box::new(transport)
        );

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Removes a transport from the protocol by type.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `transport_type` must be one of: 0 (Internet), 1 (BLE), 2 (WiFiDirect).
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_remove_transport(
    handle: *mut ProtocolHandle,
    transport_type: i32,
) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null above.
        let handle_ref = &mut *(handle as *mut ProtocolHandle);

        use offline_protocol_transport::TransportType;
        
        let transport = match transport_type {
            0 => TransportType::Internet,
            1 => TransportType::BLE,
            2 => TransportType::WiFiDirect,
            _ => return ERROR_INVALID_CONFIG,
        };

        handle_ref.protocol.transport_manager_mut().remove_transport(transport);

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets the list of active transports.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_buffer` must be a valid pointer to a buffer of at least `buffer_len` bytes.
///
/// The output format is a JSON array of transport names, e.g.:
/// `["ble", "internet"]`
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_get_active_transports(
    handle: *mut ProtocolHandle,
    out_buffer: *mut c_char,
    buffer_len: usize,
) -> i32 {
    if handle.is_null() || out_buffer.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated all pointers are non-null above.
        let handle_ref = &*handle;

        let transports = handle_ref.protocol.transport_manager().get_active_transports();
        
        let transport_names: Vec<&str> = transports.iter().map(|t| {
            use offline_protocol_transport::TransportType;
            match t {
                TransportType::Internet => "internet",
                TransportType::BLE => "ble",
                TransportType::WiFiDirect => "wifiDirect",
            }
        }).collect();

        let json = match serde_json::to_string(&transport_names) {
            Ok(j) => j,
            Err(_) => return ERROR_OTHER,
        };

        let json_cstr = match CString::new(json) {
            Ok(s) => s,
            Err(_) => return ERROR_OTHER,
        };

        let json_bytes = json_cstr.as_bytes_with_nul();
        if json_bytes.len() > buffer_len {
            return ERROR_OTHER;
        }

        ptr::copy_nonoverlapping(
            json_bytes.as_ptr(),
            out_buffer as *mut u8,
            json_bytes.len(),
        );

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets the current network topology as JSON.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_buffer` must be a valid pointer to a buffer of at least `buffer_len` bytes.
///
/// The output is a JSON string containing the complete network topology including nodes, links, and stats.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_get_topology(
    handle: *mut ProtocolHandle,
    out_buffer: *mut c_char,
    buffer_len: usize,
) -> i32 {
    if handle.is_null() || out_buffer.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated all pointers are non-null above.
        let handle_ref = &*handle;

        let visualizer = handle_ref.visualizer.lock().unwrap();
        
        let topology_json = match visualizer.export_json() {
            Ok(json) => json,
            Err(_) => return ERROR_OTHER,
        };

        let json_cstr = match CString::new(topology_json) {
            Ok(s) => s,
            Err(_) => return ERROR_OTHER,
        };

        let json_bytes = json_cstr.as_bytes_with_nul();
        if json_bytes.len() > buffer_len {
            return ERROR_OTHER;
        }

        ptr::copy_nonoverlapping(
            json_bytes.as_ptr(),
            out_buffer as *mut u8,
            json_bytes.len(),
        );

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets message delivery statistics as JSON.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_buffer` must be a valid pointer to a buffer of at least `buffer_len` bytes.
///
/// The output is a JSON array containing message statistics.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_get_message_stats(
    handle: *mut ProtocolHandle,
    out_buffer: *mut c_char,
    buffer_len: usize,
) -> i32 {
    if handle.is_null() || out_buffer.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated all pointers are non-null above.
        let handle_ref = &*handle;

        let visualizer = handle_ref.visualizer.lock().unwrap();
        let stats = visualizer.get_message_stats();
        
        let stats_json = match serde_json::to_string(&stats) {
            Ok(json) => json,
            Err(_) => return ERROR_OTHER,
        };

        let json_cstr = match CString::new(stats_json) {
            Ok(s) => s,
            Err(_) => return ERROR_OTHER,
        };

        let json_bytes = json_cstr.as_bytes_with_nul();
        if json_bytes.len() > buffer_len {
            return ERROR_OTHER;
        }

        ptr::copy_nonoverlapping(
            json_bytes.as_ptr(),
            out_buffer as *mut u8,
            json_bytes.len(),
        );

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets delivery success rate (0.0 - 1.0).
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_rate` must be a valid pointer to store the success rate.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_get_delivery_success_rate(
    handle: *mut ProtocolHandle,
    out_rate: *mut f32,
) -> i32 {
    if handle.is_null() || out_rate.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated pointers are non-null above.
        let handle_ref = &*handle;

        let visualizer = handle_ref.visualizer.lock().unwrap();
        let rate = visualizer.delivery_success_rate();
        
        *out_rate = rate;

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets median delivery latency in milliseconds.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_latency` must be a valid pointer to store the latency.
///
/// # Returns
///
/// Returns SUCCESS if latency is available, 0 if no data, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_get_median_latency(
    handle: *mut ProtocolHandle,
    out_latency: *mut u64,
) -> i32 {
    if handle.is_null() || out_latency.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated pointers are non-null above.
        let handle_ref = &*handle;

        let visualizer = handle_ref.visualizer.lock().unwrap();
        
        match visualizer.median_latency() {
            Some(latency) => {
                *out_latency = latency;
                SUCCESS
            }
            None => 0, // No data available
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Gets median hop count.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_hops` must be a valid pointer to store the hop count.
///
/// # Returns
///
/// Returns SUCCESS if hop count is available, 0 if no data, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_get_median_hops(
    handle: *mut ProtocolHandle,
    out_hops: *mut u8,
) -> i32 {
    if handle.is_null() || out_hops.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated pointers are non-null above.
        let handle_ref = &*handle;

        let visualizer = handle_ref.visualizer.lock().unwrap();
        
        match visualizer.median_hops() {
            Some(hops) => {
                *out_hops = hops;
                SUCCESS
            }
            None => 0, // No data available
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Updates transport metrics for DORS scoring.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `transport_type` must be one of: 0 (Internet), 1 (BLE), 2 (WiFiDirect).
///
/// # Arguments
///
/// - `rssi`: Signal strength in dBm (or -1 if not applicable)
/// - `latency_ms`: Latency in milliseconds (or 0 if not applicable)
/// - `bandwidth_bps`: Bandwidth in bytes per second (or 0 if not applicable)
/// - `congestion`: Congestion level from 0.0 to 1.0
/// - `queue_depth`: Number of messages in send queue
/// - `success_count`: Number of successful sends in last window
/// - `failure_count`: Number of failed sends in last window
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_update_transport_metrics(
    handle: *mut ProtocolHandle,
    transport_type: i32,
    rssi: i16,
    latency_ms: u32,
    bandwidth_bps: u64,
    congestion: f32,
    queue_depth: usize,
    success_count: u32,
    failure_count: u32,
) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null above.
        let handle_ref = &*handle;

        use offline_protocol_transport::{TransportMetrics, TransportType};
        
        let transport = match transport_type {
            0 => TransportType::Internet,
            1 => TransportType::BLE,
            2 => TransportType::WiFiDirect,
            _ => return ERROR_INVALID_CONFIG,
        };

        let metrics = TransportMetrics {
            rssi: if rssi == -1 { None } else { Some(rssi) },
            latency_ms: if latency_ms == 0 { None } else { Some(latency_ms) },
            bandwidth_bps: if bandwidth_bps == 0 { None } else { Some(bandwidth_bps) },
            congestion: congestion.clamp(0.0, 1.0),
            queue_depth,
            success_count,
            failure_count,
        };

        // Update metrics based on transport type
        match transport {
            TransportType::BLE => {
                let ble_transport_opt = handle_ref.ble_transport.lock().unwrap();
                if let Some(ref ble_transport) = *ble_transport_opt {
                    ble_transport.update_metrics(metrics);
                    SUCCESS
                } else {
                    ERROR_OTHER
                }
            }
            TransportType::Internet => {
                // Get Internet transport from protocol manager
                if let Some(_transport_arc) = handle_ref.protocol.transport_manager()
                    .get_transport(TransportType::Internet)
                {
                    // We need to downcast to access update_metrics
                    // For now, we'll just return success as the base Transport trait doesn't expose this
                    // In a real implementation, we'd need a way to update metrics on the transport
                    SUCCESS
                } else {
                    ERROR_OTHER
                }
            }
            TransportType::WiFiDirect => {
                // Similar to Internet transport
                if let Some(_transport_arc) = handle_ref.protocol.transport_manager()
                    .get_transport(TransportType::WiFiDirect)
                {
                    SUCCESS
                } else {
                    ERROR_OTHER
                }
            }
        }
    });

    result.unwrap_or(ERROR_PANIC)
}

/// Checks if DORS should escalate from BLE to Wi-Fi Direct.
///
/// # Safety
///
/// - `handle` must be a valid pointer returned by `offline_protocol_create`.
/// - `out_should_escalate` must be a valid pointer to store the result.
///
/// # Returns
///
/// Returns SUCCESS on success, or an error code on failure.
/// Sets `out_should_escalate` to 1 if escalation is needed, 0 otherwise.
#[no_mangle]
pub extern "C" fn offline_protocol_should_escalate_to_wifi(
    handle: *mut ProtocolHandle,
    out_should_escalate: *mut i32,
) -> i32 {
    if handle.is_null() || out_should_escalate.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated pointers are non-null above.
        let handle_ref = &*handle;

        // Check DORS escalation signal
        let should_escalate = handle_ref.protocol.transport_manager().should_escalate_to_wifi();
        *out_should_escalate = if should_escalate { 1 } else { 0 };

        SUCCESS
    });

    result.unwrap_or(ERROR_PANIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_create_destroy_protocol() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        assert!(!handle.is_null());

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_create_with_null_config() {
        let handle = offline_protocol_create(ptr::null());
        assert!(handle.is_null());
    }

    #[test]
    fn test_create_with_invalid_json() {
        let config = CString::new("invalid json").unwrap();
        let handle = offline_protocol_create(config.as_ptr());
        assert!(handle.is_null());
    }

    #[test]
    fn test_create_with_missing_fields() {
        let config = CString::new(r#"{"appId": "test-app"}"#).unwrap();
        let handle = offline_protocol_create(config.as_ptr());
        assert!(handle.is_null()); // Missing userId
    }

    #[test]
    fn test_event_callback() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        assert!(!handle.is_null());

        // Set up callback
        static CALLBACK_CALLED: AtomicBool = AtomicBool::new(false);

        extern "C" fn test_callback(event_json: *const c_char, _user_data: *mut c_void) {
            assert!(!event_json.is_null());
            CALLBACK_CALLED.store(true, Ordering::SeqCst);
        }

        let result = offline_protocol_set_event_callback(handle, Some(test_callback), ptr::null_mut());
        assert_eq!(result, SUCCESS);

        // Add a mock transport for testing
        unsafe {
            use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
            let protocol_ref = &mut *(handle as *mut ProtocolHandle);
            let mut mock_transport = MockTransport::new(TransportType::BLE);
            mock_transport.start().unwrap();
            protocol_ref.protocol.transport_manager_mut().add_transport(
                TransportType::BLE, 
                Box::new(mock_transport)
            );
        }

        // Start protocol and send message (which should trigger callback)
        offline_protocol_start(handle);
        
        let recipient = CString::new("bob").unwrap();
        let content = CString::new("Hello!").unwrap();
        let mut out_buffer = vec![0u8; 256];

        offline_protocol_send_message(
            handle,
            recipient.as_ptr(),
            content.as_ptr(),
            1,
            out_buffer.as_mut_ptr() as *mut c_char,
            out_buffer.len(),
        );

        // Give callback a moment to execute
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Verify callback was called
        assert!(CALLBACK_CALLED.load(Ordering::SeqCst));

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_start_stop() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        assert!(!handle.is_null());

        let result = offline_protocol_start(handle);
        assert_eq!(result, SUCCESS);

        let result = offline_protocol_stop(handle);
        assert_eq!(result, SUCCESS);

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_start_with_null_handle() {
        let result = offline_protocol_start(ptr::null_mut());
        assert_eq!(result, ERROR_NULL_POINTER);
    }

    #[test]
    fn test_double_start() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        
        offline_protocol_start(handle);
        let result = offline_protocol_start(handle);
        assert_eq!(result, ERROR_ALREADY_STARTED);

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_send_message() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        
        // Add a mock transport for testing
        unsafe {
            use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
            let protocol_ref = &mut *(handle as *mut ProtocolHandle);
            let mut mock_transport = MockTransport::new(TransportType::BLE);
            mock_transport.start().unwrap();
            protocol_ref.protocol.transport_manager_mut().add_transport(
                TransportType::BLE, 
                Box::new(mock_transport)
            );
        }
        
        offline_protocol_start(handle);

        let recipient = CString::new("bob").unwrap();
        let content = CString::new("Hello!").unwrap();
        let mut out_buffer = vec![0u8; 256];

        let result = offline_protocol_send_message(
            handle,
            recipient.as_ptr(),
            content.as_ptr(),
            1, // Medium priority
            out_buffer.as_mut_ptr() as *mut c_char,
            out_buffer.len(),
        );

        assert_eq!(result, SUCCESS);

        // Check that message ID was written
        let msg_id = unsafe { CStr::from_ptr(out_buffer.as_ptr() as *const c_char) };
        let msg_id_str = msg_id.to_str().unwrap();
        assert!(!msg_id_str.is_empty());

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_send_message_not_started() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        // Don't start

        let recipient = CString::new("bob").unwrap();
        let content = CString::new("Hello!").unwrap();
        let mut out_buffer = vec![0u8; 256];

        let result = offline_protocol_send_message(
            handle,
            recipient.as_ptr(),
            content.as_ptr(),
            1,
            out_buffer.as_mut_ptr() as *mut c_char,
            out_buffer.len(),
        );

        assert_eq!(result, ERROR_NOT_STARTED);

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_send_message_with_null_params() {
        let config = r#"{"appId": "test-app", "userId": "user123"}"#;
        let config_c = CString::new(config).unwrap();

        let handle = offline_protocol_create(config_c.as_ptr());
        offline_protocol_start(handle);

        let mut out_buffer = vec![0u8; 256];

        // Null recipient
        let result = offline_protocol_send_message(
            handle,
            ptr::null(),
            CString::new("Hello").unwrap().as_ptr(),
            1,
            out_buffer.as_mut_ptr() as *mut c_char,
            out_buffer.len(),
        );
        assert_eq!(result, ERROR_NULL_POINTER);

        offline_protocol_destroy(handle);
    }

    #[test]
    fn test_destroy_null_handle() {
        // Should not crash
        offline_protocol_destroy(ptr::null_mut());
    }

    #[test]
    fn test_free_null_string() {
        // Should not crash
        offline_protocol_free_string(ptr::null_mut());
    }
}
