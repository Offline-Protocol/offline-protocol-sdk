//! C-compatible FFI layer for Offline Protocol SDK
//!
//! This crate provides C-compatible bindings that can be used from other languages
//! including TypeScript (via napi-rs), Swift (iOS), and Java/Kotlin (Android).

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic;
use std::ptr;

// Opaque pointer type for OfflineProtocol
pub struct OfflineProtocolHandle {
    _private: [u8; 0],
}

/// Error codes for FFI operations
#[repr(C)]
pub enum ErrorCode {
    Success = 0,
    InvalidArgument = 1,
    NotStarted = 2,
    AlreadyStarted = 3,
    SendFailed = 4,
    PermissionDenied = 5,
    Unknown = 99,
}

/// Create a new OfflineProtocol instance
///
/// # Safety
/// - `config_json` must be a valid null-terminated UTF-8 string
/// - The returned handle must be freed with `offline_protocol_free`
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_new(
    config_json: *const c_char,
) -> *mut OfflineProtocolHandle {
    let result = panic::catch_unwind(|| {
        if config_json.is_null() {
            return ptr::null_mut();
        }

        let _config_str = match CStr::from_ptr(config_json).to_str() {
            Ok(s) => s,
            Err(_) => return ptr::null_mut(),
        };

        // TODO: Parse config JSON and create OfflineProtocol instance
        // For now, return a placeholder
        ptr::null_mut()
    });

    result.unwrap_or(ptr::null_mut())
}

/// Free an OfflineProtocol instance
///
/// # Safety
/// - `handle` must be a valid pointer returned from `offline_protocol_new`
/// - `handle` must not be used after this call
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_free(handle: *mut OfflineProtocolHandle) {
    if handle.is_null() {
        return;
    }

    let _ = panic::catch_unwind(|| {
        // TODO: Drop the OfflineProtocol instance
    });
}

/// Start the protocol
///
/// # Safety
/// - `handle` must be a valid pointer returned from `offline_protocol_new`
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_start(
    handle: *mut OfflineProtocolHandle,
) -> ErrorCode {
    if handle.is_null() {
        return ErrorCode::InvalidArgument;
    }

    let result = panic::catch_unwind(|| {
        // TODO: Start the protocol
        ErrorCode::Success
    });

    result.unwrap_or(ErrorCode::Unknown)
}

/// Stop the protocol
///
/// # Safety
/// - `handle` must be a valid pointer returned from `offline_protocol_new`
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_stop(
    handle: *mut OfflineProtocolHandle,
) -> ErrorCode {
    if handle.is_null() {
        return ErrorCode::InvalidArgument;
    }

    let result = panic::catch_unwind(|| {
        // TODO: Stop the protocol
        ErrorCode::Success
    });

    result.unwrap_or(ErrorCode::Unknown)
}

/// Send a message
///
/// # Safety
/// - `handle` must be a valid pointer
/// - `message_json` must be a valid null-terminated UTF-8 string
/// - Returns a message ID as a JSON string (caller must free with `offline_protocol_free_string`)
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_send_message(
    handle: *mut OfflineProtocolHandle,
    message_json: *const c_char,
) -> *mut c_char {
    if handle.is_null() || message_json.is_null() {
        return ptr::null_mut();
    }

    let result = panic::catch_unwind(|| {
        let _message_str = match CStr::from_ptr(message_json).to_str() {
            Ok(s) => s,
            Err(_) => return ptr::null_mut(),
        };

        // TODO: Parse message JSON and send
        // Return message ID as JSON string
        ptr::null_mut()
    });

    result.unwrap_or(ptr::null_mut())
}

/// Free a string returned by the FFI
///
/// # Safety
/// - `s` must be a string returned from an FFI function
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }

    let _ = panic::catch_unwind(|| {
        let _ = CString::from_raw(s);
    });
}

/// Get the library version
///
/// # Safety
/// - Returns a static string, no need to free
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_version() -> *const c_char {
    const VERSION: &str = "0.1.0\0";
    VERSION.as_ptr() as *const c_char
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        unsafe {
            let version = offline_protocol_version();
            assert!(!version.is_null());
            let version_str = CStr::from_ptr(version).to_str().unwrap();
            assert!(version_str.starts_with("0.1"));
        }
    }
}

