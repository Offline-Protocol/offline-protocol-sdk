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

use offline_protocol::{OfflineProtocol, ProtocolConfig};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic;
use std::ptr;

/// Success code.
pub const SUCCESS: i32 = 0;

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

/// Creates a new OfflineProtocol instance from JSON configuration.
///
/// # Safety
///
/// `config_json` must be a valid null-terminated C string.
/// Returns a pointer to OfflineProtocol on success, or null on failure.
#[no_mangle]
pub extern "C" fn offline_protocol_create(config_json: *const c_char) -> *mut OfflineProtocol {
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

        // Create protocol config
        let protocol_config = ProtocolConfig::new(app_id, user_id);

        // Create protocol instance
        match OfflineProtocol::new(protocol_config) {
            Ok(protocol) => Box::into_raw(Box::new(protocol)),
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
pub extern "C" fn offline_protocol_destroy(handle: *mut OfflineProtocol) {
    if handle.is_null() {
        return;
    }

    unsafe {
        // SAFETY: We validated handle is non-null. The caller must ensure
        // this is a valid OfflineProtocol pointer created by offline_protocol_create.
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
pub extern "C" fn offline_protocol_start(handle: *mut OfflineProtocol) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null and it was created by
        // offline_protocol_create, so we know it's a valid OfflineProtocol.
        let protocol = &mut *handle;

        match protocol.start() {
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
pub extern "C" fn offline_protocol_stop(handle: *mut OfflineProtocol) -> i32 {
    if handle.is_null() {
        return ERROR_NULL_POINTER;
    }

    let result = panic::catch_unwind(|| unsafe {
        // SAFETY: We validated handle is non-null and it was created by
        // offline_protocol_create, so we know it's a valid OfflineProtocol.
        let protocol = &mut *handle;

        match protocol.stop() {
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
    handle: *mut OfflineProtocol,
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
        let protocol = &mut *handle;

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
        let message_id = match protocol.send_message(recipient_str, content_str, Some(priority)) {
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
    handle: *mut OfflineProtocol,
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
