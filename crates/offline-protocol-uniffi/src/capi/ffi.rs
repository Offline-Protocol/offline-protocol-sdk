//! The `extern "C"` functions. Everything here is a thin shell over the safe
//! dispatcher in `mod.rs`: check pointers, decode UTF-8, catch panics, hand
//! out strings the caller frees.
//!
//! Every function catches a panic and answers `Internal`. That is not
//! defensive habit: a panic that reaches an `extern "C"` boundary aborts the
//! whole host process, which for a C host embedding the SDK is a crash in
//! someone else's program. A build under `panic = "abort"` (the `minisize`
//! profile) aborts before anything can be caught; the desktop libraries the
//! release ships unwind.

use core::ffi::{c_char, c_void};
use std::any::Any;
use std::ffi::{CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::adapters;
pub use super::adapters::{
    OfflineProtocolDropFn, OfflineProtocolEventFn, OfflineProtocolNotifyFn, OfflineProtocolStorage,
    OfflineProtocolStorageStatus,
};
use super::{call_static, CallError, Engine};

/// An engine instance. Opaque. Created by `offline_protocol_create`, freed by
/// `offline_protocol_free`, and usable from any thread in between.
pub struct OfflineProtocolHandle {
    engine: Engine,
}

/// Why a call produced no result. `Ok` is zero.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineProtocolStatus {
    /// Success.
    Ok = 0,
    /// The engine refused. `variant` names the definition's `ProtocolError`
    /// variant and `code` is its zero-based position.
    EngineError = 1,
    /// A null pointer where one is required, text that is not UTF-8, or
    /// parameters that are not JSON.
    InvalidArgument = 2,
    /// No method of that name, or one that has its own entry point.
    MethodNotFound = 3,
    /// The parameters are JSON and do not fit the declaration.
    InvalidParams = 4,
    /// The library failed: a panic caught at the boundary.
    Internal = 5,
}

/// The transports that notify the host of outbound frames, for
/// `offline_protocol_set_transport_callback`. The Internet transport has no
/// callback in the definition; the host polls it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineProtocolTransport {
    /// `set_ble_transport_callback`.
    Ble = 0,
    /// `set_wifi_direct_transport_callback`.
    WifiDirect = 1,
    /// `set_reticulum_transport_callback`.
    Reticulum = 2,
    /// `set_nostr_transport_callback`.
    Nostr = 3,
}

/// A refusal. Written only when a call fails, and never read by the
/// library, so a caller that reuses one clears it first with
/// `offline_protocol_error_clear`.
#[repr(C)]
#[derive(Debug)]
pub struct OfflineProtocolError {
    /// The same value the call returned.
    pub status: OfflineProtocolStatus,
    /// For `EngineError`, the variant's zero-based position in the
    /// definition's `ProtocolError` (the local API's `-32000 - code`);
    /// otherwise -1.
    pub code: i32,
    /// For `EngineError`, the variant's name, such as `"MlsNotInitialized"`;
    /// otherwise null. Owned by the caller.
    pub variant: *mut c_char,
    /// A human-readable reason. Owned by the caller. Null only if the reason
    /// could not be represented.
    pub message: *mut c_char,
}

fn status_of(error: &CallError) -> OfflineProtocolStatus {
    match error {
        CallError::Engine(_) => OfflineProtocolStatus::EngineError,
        CallError::InvalidArgument(_) => OfflineProtocolStatus::InvalidArgument,
        CallError::MethodNotFound(_) => OfflineProtocolStatus::MethodNotFound,
        CallError::InvalidParams(_) => OfflineProtocolStatus::InvalidParams,
        CallError::Internal(_) => OfflineProtocolStatus::Internal,
    }
}

/// A string the caller frees with `offline_protocol_string_free`, or null
/// when it holds a NUL byte (which JSON and the engine's messages never do).
fn owned(text: String) -> *mut c_char {
    CString::new(text).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_owned());
    format!("the library panicked: {detail}")
}

/// Runs `body`, catching a panic, and reports a failure through `err`.
///
/// # Safety
///
/// `err` is null or points to writable memory for one error.
unsafe fn boundary(
    err: *mut OfflineProtocolError,
    body: impl FnOnce() -> Result<(), CallError>,
) -> OfflineProtocolStatus {
    let outcome = catch_unwind(AssertUnwindSafe(body))
        .unwrap_or_else(|payload| Err(CallError::Internal(panic_message(payload.as_ref()))));
    match outcome {
        Ok(()) => OfflineProtocolStatus::Ok,
        Err(error) => {
            let status = status_of(&error);
            if !err.is_null() {
                let (variant, code) = error
                    .engine_variant()
                    .map_or((std::ptr::null_mut(), -1), |(name, code)| {
                        (owned(name.to_owned()), code)
                    });
                // SAFETY: the caller's error, checked non-null above.
                unsafe {
                    err.write(OfflineProtocolError {
                        status,
                        code,
                        variant,
                        message: owned(error.message()),
                    });
                }
            }
            status
        }
    }
}

/// [`boundary`], for the test that a panic becomes `Internal`.
///
/// # Safety
///
/// As [`boundary`].
#[cfg(test)]
pub(super) unsafe fn boundary_for_test(
    err: *mut OfflineProtocolError,
    body: impl FnOnce() -> Result<(), CallError>,
) -> OfflineProtocolStatus {
    // SAFETY: as the caller promised.
    unsafe { boundary(err, body) }
}

/// The text at `ptr`, which must not be null.
///
/// # Safety
///
/// `ptr` is null or a NUL-terminated string valid for the call.
unsafe fn text<'a>(ptr: *const c_char, what: &str) -> Result<&'a str, CallError> {
    if ptr.is_null() {
        return Err(CallError::InvalidArgument(format!("{what} is null")));
    }
    // SAFETY: the caller's NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| CallError::InvalidArgument(format!("{what} is not UTF-8")))
}

/// The text at `ptr`, or `None` for null.
///
/// # Safety
///
/// As [`text`].
unsafe fn optional_text<'a>(ptr: *const c_char, what: &str) -> Result<Option<&'a str>, CallError> {
    if ptr.is_null() {
        Ok(None)
    } else {
        // SAFETY: as the caller promised.
        unsafe { text(ptr, what) }.map(Some)
    }
}

/// # Safety
///
/// `handle` is null or a live handle.
unsafe fn engine<'a>(handle: *const OfflineProtocolHandle) -> Result<&'a Engine, CallError> {
    // SAFETY: the caller's handle, from `offline_protocol_create`.
    unsafe { handle.as_ref() }
        .map(|h| &h.engine)
        .ok_or_else(|| CallError::InvalidArgument("the handle is null".to_owned()))
}

/// # Safety
///
/// `out` is null or writable.
unsafe fn put<T>(out: *mut T, value: T, what: &str) -> Result<(), CallError> {
    if out.is_null() {
        return Err(CallError::InvalidArgument(format!("{what} is null")));
    }
    // SAFETY: checked non-null; the caller promised it is writable.
    unsafe { out.write(value) };
    Ok(())
}

/// The library's version, the release it was built from, such as
/// `"0.29.0"`. Static: never free it.
#[no_mangle]
pub extern "C" fn offline_protocol_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Calls one of the definition's instance-less functions
/// (`derive_address`, `parse_invite`, `verify_identity_assertion`), which
/// need no engine and may run before `offline_protocol_create`.
///
/// `params_json` is a JSON object keyed by the definition's parameter names,
/// or null for none. On success `*out_result` is the result as JSON, freed
/// with `offline_protocol_string_free`. On failure `*out_result` is left as
/// it was: free it only after a call returned `Ok`.
///
/// # Safety
///
/// `method` and `params_json` are null or NUL-terminated strings;
/// `out_result` is writable; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_call_static(
    method: *const c_char,
    params_json: *const c_char,
    out_result: *mut *mut c_char,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            let method = text(method, "method")?;
            let params = optional_text(params_json, "params_json")?;
            let result = call_static(method, params)?;
            put(out_result, owned(result), "out_result")
        })
    }
}

/// Creates an engine from a `ProtocolConfig` given as a JSON object. Fields
/// the definition gives a default may be omitted. On success `*out_handle`
/// is the engine, freed with `offline_protocol_free`. On failure it is left
/// as it was.
///
/// # Safety
///
/// `config_json` is a NUL-terminated string; `out_handle` is writable;
/// `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_create(
    config_json: *const c_char,
    out_handle: *mut *mut OfflineProtocolHandle,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            if out_handle.is_null() {
                return Err(CallError::InvalidArgument("out_handle is null".to_owned()));
            }
            let config = text(config_json, "config_json")?;
            let engine = Engine::create(config)?;
            let handle = Box::into_raw(Box::new(OfflineProtocolHandle { engine }));
            put(out_handle, handle, "out_handle")
        })
    }
}

/// Frees an engine. Null is ignored. No call may be in flight on the handle,
/// and the handle may not be used afterwards. Callbacks the engine held are
/// dropped, each calling its `drop` once.
///
/// # Safety
///
/// `handle` is null or a live handle no other thread is using.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_free(handle: *mut OfflineProtocolHandle) {
    if handle.is_null() {
        return;
    }
    // SAFETY: the caller's handle, from `Box::into_raw` in create.
    let owned = unsafe { Box::from_raw(handle) };
    let _ = catch_unwind(AssertUnwindSafe(move || drop(owned)));
}

/// Calls a method of the engine, of its mesh services (`services.` prefix),
/// of its document store (`data.` prefix), or an instance-less function,
/// named as the local API names it.
///
/// `params_json` is a JSON object keyed by the definition's parameter names,
/// or null for none. On success `*out_result` is the result as JSON (`null`
/// for a method that returns nothing), freed with
/// `offline_protocol_string_free`. On failure `*out_result` is left as it
/// was: free it only after a call returned `Ok`, or reset it to null after
/// each free.
///
/// # Safety
///
/// `handle` is a live handle; `method` and `params_json` are null or
/// NUL-terminated strings; `out_result` is writable; `err` is null or
/// writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_call(
    handle: *const OfflineProtocolHandle,
    method: *const c_char,
    params_json: *const c_char,
    out_result: *mut *mut c_char,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            let engine = engine(handle)?;
            let method = text(method, "method")?;
            let params = optional_text(params_json, "params_json")?;
            if out_result.is_null() {
                return Err(CallError::InvalidArgument("out_result is null".to_owned()));
            }
            let result = engine.call(method, params)?;
            put(out_result, owned(result), "out_result")
        })
    }
}

/// Installs the event callback, replacing any earlier one (whose `drop` is
/// called once it is no longer running). A null `on_event` stops delivery.
/// Every event is also queued for `offline_protocol_poll_event` either way.
///
/// # Safety
///
/// `handle` is a live handle; `on_event` and `drop` are null or functions
/// callable from any thread with `ctx`; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_set_event_callback(
    handle: *const OfflineProtocolHandle,
    on_event: OfflineProtocolEventFn,
    drop: OfflineProtocolDropFn,
    ctx: *mut c_void,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            let engine = engine(handle)?;
            engine
                .protocol()
                .set_event_callback(adapters::event_callback(on_event, drop, ctx));
            Ok(())
        })
    }
}

/// The oldest queued event as JSON, or null when none is queued. The queue
/// keeps the newest 1000. Freed with `offline_protocol_string_free`.
///
/// # Safety
///
/// `handle` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_poll_event(
    handle: *const OfflineProtocolHandle,
) -> *mut c_char {
    // SAFETY: the caller's handle.
    let Ok(engine) = (unsafe { engine(handle) }) else {
        return std::ptr::null_mut();
    };
    catch_unwind(AssertUnwindSafe(|| engine.protocol().poll_event()))
        .ok()
        .flatten()
        .map_or(std::ptr::null_mut(), owned)
}

/// Installs a transport's outbound notification, replacing any earlier one.
/// `transport` is an `OfflineProtocolTransport` value.
///
/// # Safety
///
/// `handle` is a live handle; `notify` and `drop` are null or functions
/// callable from any thread with `ctx`; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_set_transport_callback(
    handle: *const OfflineProtocolHandle,
    transport: i32,
    notify: OfflineProtocolNotifyFn,
    drop: OfflineProtocolDropFn,
    ctx: *mut c_void,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            let engine = engine(handle)?;
            let callback = adapters::notify_callback(notify, drop, ctx);
            let protocol = engine.protocol();
            // An `i32` rather than the enum type: a value C passes outside the
            // enum's range would be undefined behaviour as a Rust enum.
            match transport {
                t if t == OfflineProtocolTransport::Ble as i32 => {
                    protocol.set_ble_transport_callback(Box::new(callback));
                }
                t if t == OfflineProtocolTransport::WifiDirect as i32 => {
                    protocol.set_wifi_direct_transport_callback(Box::new(callback));
                }
                t if t == OfflineProtocolTransport::Reticulum as i32 => {
                    protocol.set_reticulum_transport_callback(Box::new(callback));
                }
                t if t == OfflineProtocolTransport::Nostr as i32 => {
                    protocol.set_nostr_transport_callback(Box::new(callback));
                }
                other => {
                    // Dropping the unused callback calls the host's drop, so
                    // a refused registration still releases `ctx`.
                    std::mem::drop(callback);
                    return Err(CallError::InvalidArgument(format!(
                        "{other} is not an OfflineProtocolTransport"
                    )));
                }
            }
            Ok(())
        })
    }
}

/// # Safety
///
/// `provider` is null or points to a provider valid for the call.
unsafe fn provider(
    provider: *const OfflineProtocolStorage,
    what: &str,
) -> Result<adapters::Storage, CallError> {
    // SAFETY: the caller's provider.
    let vt = unsafe { provider.as_ref() }
        .ok_or_else(|| CallError::InvalidArgument(format!("{what} is null")))?;
    adapters::storage(vt).map_err(|m| CallError::InvalidArgument(format!("{what}: {m}")))
}

/// Attaches the two storage providers, as `initialize_mls`: `secure_storage`
/// for key material (back it with the platform's keystore) and
/// `protocol_state_storage` for install-scoped state. Same lifecycle as
/// every binding: before `start`, idempotent. The library copies both
/// structs; each provider's `drop` is called once when the engine lets go.
///
/// `initialize_mls_with_file_stores` needs no provider and is reached through
/// `offline_protocol_call`.
///
/// # Safety
///
/// `handle` is a live handle; both providers point to valid providers whose
/// functions are callable from any thread; `err` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_initialize_mls(
    handle: *const OfflineProtocolHandle,
    secure_storage: *const OfflineProtocolStorage,
    protocol_state_storage: *const OfflineProtocolStorage,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            let engine = engine(handle)?;
            let secure = provider(secure_storage, "secure_storage")?;
            let state = provider(protocol_state_storage, "protocol_state_storage")?;
            engine
                .protocol()
                .initialize_mls(Box::new(secure), Box::new(state))?;
            Ok(())
        })
    }
}

/// Runs the storage adapter conformance suite against a protocol-state
/// provider, as `run_storage_conformance`, and returns the report as JSON:
/// `{"passed":[...],"failures":[{"check","detail"}]}`. Green is the
/// definition of a supported backend. Probe a new backend on a scratch
/// instance: the suite writes and deletes its own probe keys. On failure
/// `*out_result` is left as it was.
///
/// # Safety
///
/// `storage` points to a valid provider; `out_result` is writable; `err` is
/// null or writable.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_run_storage_conformance(
    storage: *const OfflineProtocolStorage,
    out_result: *mut *mut c_char,
    err: *mut OfflineProtocolError,
) -> OfflineProtocolStatus {
    // SAFETY: every pointer is the caller's, as documented.
    unsafe {
        boundary(err, || {
            let state = provider(storage, "storage")?;
            if out_result.is_null() {
                return Err(CallError::InvalidArgument("out_result is null".to_owned()));
            }
            let report = crate::run_storage_conformance(Box::new(state));
            put(out_result, owned(report), "out_result")
        })
    }
}

/// Frees a string the library handed out. Null is ignored.
///
/// # Safety
///
/// `s` is null or a string from this library, not yet freed.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: a string from `CString::into_raw` in this library.
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Frees an error's strings and resets it, so it can be reused. Null is
/// ignored.
///
/// # Safety
///
/// `err` is null or an error this library wrote, or one zero-initialised.
#[no_mangle]
pub unsafe extern "C" fn offline_protocol_error_clear(err: *mut OfflineProtocolError) {
    // SAFETY: the caller's error.
    let Some(error) = (unsafe { err.as_mut() }) else {
        return;
    };
    // SAFETY: both strings came from this library or are null.
    unsafe {
        offline_protocol_string_free(error.variant);
        offline_protocol_string_free(error.message);
    }
    error.variant = std::ptr::null_mut();
    error.message = std::ptr::null_mut();
    error.code = -1;
    error.status = OfflineProtocolStatus::Ok;
}
