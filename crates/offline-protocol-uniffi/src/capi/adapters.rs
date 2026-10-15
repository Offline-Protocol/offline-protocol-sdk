//! The host's C functions, behind the wrapper's callback traits.
//!
//! Each adapter holds a function pointer and the host's `ctx`, and promises
//! Rust that both may be used from any thread (`Send + Sync`). That promise
//! is the host's to keep, and the header states it on every type: the engine
//! calls storage and event callbacks from whichever thread is inside it, and
//! from the telemetry uploader's own thread.
//!
//! When the library lets go of an adapter it calls the host's `drop`, once,
//! and never touches `ctx` again. A replaced event callback may still be
//! running on another thread when it is replaced; the drop comes after that
//! call returns, which is the only point at which freeing `ctx` is safe.

use core::ffi::{c_char, c_void};
use std::ffi::CString;

use crate::{
    BleTransportCallback, EventCallback, MlsStorageError, MlsStorageProvider,
    NostrTransportCallback, ProtocolStateStorageProvider, ReticulumTransportCallback,
    WifiDirectTransportCallback,
};

/// Receives one engine event as JSON, the same text `poll_event` returns.
///
/// Runs synchronously on the thread that produced the event, inside the
/// engine call that produced it. Calling back into the engine from it is
/// unsupported: queue the text and return. `event_json` is valid only for
/// the duration of the call. Must not unwind (no C++ exception, no
/// `longjmp`) out of the call.
pub type OfflineProtocolEventFn =
    Option<unsafe extern "C" fn(ctx: *mut c_void, event_json: *const c_char)>;

/// Says a transport has outbound frames for the host to drain. Same thread
/// and re-entrancy rules as [`OfflineProtocolEventFn`].
pub type OfflineProtocolNotifyFn = Option<unsafe extern "C" fn(ctx: *mut c_void)>;

/// Called once when the library no longer holds `ctx`. May be null.
pub type OfflineProtocolDropFn = Option<unsafe extern "C" fn(ctx: *mut c_void)>;

/// What a storage function answers. The nonzero values are the definition's
/// `MlsStorageError` variants, in its order, so a value means the same
/// thing here as in every other binding.
///
/// Report `CorruptedData` only for a record no later launch can read. A
/// locked keystore or any I/O failure is `LoadFailed`: the engine answers
/// `CorruptedData` on the protocol-state record key by minting a new key,
/// which strands every record sealed under the old one. Any value other than
/// these six is read as the failing operation's own variant (`StoreFailed`
/// for `store`, and so on), never as `CorruptedData`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineProtocolStorageStatus {
    /// Success.
    Ok = 0,
    /// The write did not happen.
    StoreFailed = 1,
    /// The read failed and may succeed later.
    LoadFailed = 2,
    /// The delete failed.
    DeleteFailed = 3,
    /// The key is not there, where the operation required it to be.
    KeyNotFound = 4,
    /// The record is permanently unreadable. See above before using this.
    CorruptedData = 5,
}

/// A host storage provider: the same four operations every binding's
/// provider implements, as C functions.
///
/// Values are opaque bytes, stored and returned verbatim. `load` and
/// `list_keys` hand back a buffer the host allocated; the library copies it
/// and returns it through `release`. `load` answers a key that was never
/// written with success and `*out_data = NULL`. `list_keys` answers a JSON
/// array of strings, as bytes.
///
/// For the protocol-state provider, `load` must bound its read: check the
/// stored size first and never hand back more than the protocol-state
/// record ceiling (8 MiB). A larger entry is corrupt; drop it and answer
/// `CorruptedData`. See `ProtocolStateStorageProvider` in the interface
/// definition.
///
/// Every function may be called from any thread, concurrently.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OfflineProtocolStorage {
    /// Passed as the first argument to every function below.
    pub ctx: *mut c_void,
    /// Writes `data[0..len]` under the key, replacing any previous value.
    pub store: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            key_type: *const c_char,
            key_id: *const c_char,
            data: *const u8,
            len: usize,
        ) -> i32,
    >,
    /// Reads the key into a host buffer, or `NULL` for absent.
    pub load: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            key_type: *const c_char,
            key_id: *const c_char,
            out_data: *mut *mut u8,
            out_len: *mut usize,
        ) -> i32,
    >,
    /// Removes the key. Removing a key that is not there is success.
    /// (Not `delete`, which C++ reserves.)
    pub remove: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            key_type: *const c_char,
            key_id: *const c_char,
        ) -> i32,
    >,
    /// Writes the key ids under `key_type` as a JSON array of strings.
    pub list_keys: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            key_type: *const c_char,
            out_json: *mut *mut u8,
            out_len: *mut usize,
        ) -> i32,
    >,
    /// Frees a buffer `load` or `list_keys` handed back. May be null when
    /// the host's buffers need no freeing.
    pub release: Option<unsafe extern "C" fn(ctx: *mut c_void, data: *mut u8, len: usize)>,
    /// Called once when the library no longer holds this provider. May be
    /// null.
    pub drop: OfflineProtocolDropFn,
}

/// Holds a host function and its context. See the module documentation for
/// the threading promise.
pub(super) struct Host<F> {
    f: F,
    drop: OfflineProtocolDropFn,
    ctx: *mut c_void,
}

// SAFETY: the header requires every function and `ctx` handed to the library
// to be usable from any thread, concurrently. That is the whole contract of
// these types, and the library has no way to check it.
unsafe impl<F> Send for Host<F> {}
// SAFETY: as above.
unsafe impl<F> Sync for Host<F> {}

impl<F> Drop for Host<F> {
    fn drop(&mut self) {
        if let Some(drop) = self.drop {
            // SAFETY: called once, with the host's own context, after the
            // library's last use of it.
            unsafe { drop(self.ctx) }
        }
    }
}

/// An event callback. A null function installs one that does nothing,
/// which is how a host stops receiving events.
pub(super) fn event_callback(
    f: OfflineProtocolEventFn,
    drop: OfflineProtocolDropFn,
    ctx: *mut c_void,
) -> Box<dyn EventCallback> {
    Box::new(Host { f, drop, ctx })
}

impl EventCallback for Host<OfflineProtocolEventFn> {
    fn on_event(&self, event_json: String) {
        let Some(f) = self.f else { return };
        // serde_json escapes U+0000, so an event never holds a NUL byte.
        let Ok(text) = CString::new(event_json) else {
            return;
        };
        // SAFETY: the host's function, with its context and a string that
        // lives for the call.
        unsafe { f(self.ctx, text.as_ptr()) }
    }
}

/// A transport notification callback.
pub(super) fn notify_callback(
    f: OfflineProtocolNotifyFn,
    drop: OfflineProtocolDropFn,
    ctx: *mut c_void,
) -> Host<OfflineProtocolNotifyFn> {
    Host { f, drop, ctx }
}

impl Host<OfflineProtocolNotifyFn> {
    fn notify(&self) {
        if let Some(f) = self.f {
            // SAFETY: the host's function with its own context.
            unsafe { f(self.ctx) }
        }
    }
}

impl BleTransportCallback for Host<OfflineProtocolNotifyFn> {
    fn on_fragments_available(&self) {
        self.notify();
    }
}

impl WifiDirectTransportCallback for Host<OfflineProtocolNotifyFn> {
    fn on_messages_available(&self) {
        self.notify();
    }
}

impl ReticulumTransportCallback for Host<OfflineProtocolNotifyFn> {
    fn on_messages_available(&self) {
        self.notify();
    }
}

impl NostrTransportCallback for Host<OfflineProtocolNotifyFn> {
    fn on_messages_available(&self) {
        self.notify();
    }
}

/// A host storage provider, as either of the wrapper's provider traits.
pub(super) struct Storage(Host<OfflineProtocolStorage>);

/// Why a provider cannot be used at all.
pub(super) fn storage(provider: &OfflineProtocolStorage) -> Result<Storage, &'static str> {
    if provider.store.is_none()
        || provider.load.is_none()
        || provider.remove.is_none()
        || provider.list_keys.is_none()
    {
        return Err("a storage provider needs store, load, remove and list_keys");
    }
    Ok(Storage(Host {
        f: *provider,
        drop: provider.drop,
        ctx: provider.ctx,
    }))
}

/// The variant a host's answer names, or `own` for a value outside the
/// definition's range. Never `CorruptedData` by default: that answer
/// destroys recoverable state, so only the host may give it.
fn answer(code: i32, own: fn() -> MlsStorageError) -> Result<(), MlsStorageError> {
    if code == OfflineProtocolStorageStatus::Ok as i32 {
        return Ok(());
    }
    Err(super::generated::mls_storage_error_from_code(code).unwrap_or_else(own))
}

fn key(text: String, own: fn() -> MlsStorageError) -> Result<CString, MlsStorageError> {
    CString::new(text).map_err(|_| own())
}

impl Storage {
    fn vt(&self) -> &OfflineProtocolStorage {
        &self.0.f
    }

    /// Takes ownership of a buffer the host handed back.
    fn take_buffer(&self, data: *mut u8, len: usize) -> Option<Vec<u8>> {
        if data.is_null() {
            return None;
        }
        // SAFETY: the host promised `len` readable bytes at `data` until
        // `release`, which follows the copy.
        let copy = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        if let Some(release) = self.vt().release {
            // SAFETY: the host's own buffer, returned once.
            unsafe { release(self.0.ctx, data, len) }
        }
        Some(copy)
    }

    fn store_bytes(
        &self,
        key_type: String,
        key_id: String,
        data: &[u8],
    ) -> Result<(), MlsStorageError> {
        let own = || MlsStorageError::StoreFailed;
        let (t, i) = (key(key_type, own)?, key(key_id, own)?);
        let f = self.vt().store.ok_or_else(own)?;
        // SAFETY: the host's function; the strings and the slice outlive it.
        answer(
            unsafe {
                f(
                    self.0.ctx,
                    t.as_ptr(),
                    i.as_ptr(),
                    data.as_ptr(),
                    data.len(),
                )
            },
            own,
        )
    }

    fn load_bytes(
        &self,
        key_type: String,
        key_id: String,
    ) -> Result<Option<Vec<u8>>, MlsStorageError> {
        let own = || MlsStorageError::LoadFailed;
        let (t, i) = (key(key_type, own)?, key(key_id, own)?);
        let f = self.vt().load.ok_or_else(own)?;
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: the host's function, with valid out pointers.
        let code = unsafe { f(self.0.ctx, t.as_ptr(), i.as_ptr(), &mut data, &mut len) };
        let taken = self.take_buffer(data, len);
        answer(code, own)?;
        Ok(taken)
    }

    fn remove_key(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError> {
        let own = || MlsStorageError::DeleteFailed;
        let (t, i) = (key(key_type, own)?, key(key_id, own)?);
        let f = self.vt().remove.ok_or_else(own)?;
        // SAFETY: the host's function; the strings outlive it.
        answer(unsafe { f(self.0.ctx, t.as_ptr(), i.as_ptr()) }, own)
    }

    fn keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError> {
        let own = || MlsStorageError::LoadFailed;
        let t = key(key_type, own)?;
        let f = self.vt().list_keys.ok_or_else(own)?;
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: the host's function, with valid out pointers.
        let code = unsafe { f(self.0.ctx, t.as_ptr(), &mut data, &mut len) };
        let taken = self.take_buffer(data, len);
        answer(code, own)?;
        match taken {
            None => Ok(Vec::new()),
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| own()),
        }
    }
}

impl MlsStorageProvider for Storage {
    fn store(
        &self,
        key_type: String,
        key_id: String,
        data: Vec<u8>,
    ) -> Result<(), MlsStorageError> {
        self.store_bytes(key_type, key_id, &data)
    }
    fn load(&self, key_type: String, key_id: String) -> Result<Option<Vec<u8>>, MlsStorageError> {
        self.load_bytes(key_type, key_id)
    }
    fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError> {
        self.remove_key(key_type, key_id)
    }
    fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError> {
        self.keys(key_type)
    }
}

impl ProtocolStateStorageProvider for Storage {
    fn store(
        &self,
        key_type: String,
        key_id: String,
        data: Vec<u8>,
    ) -> Result<(), MlsStorageError> {
        self.store_bytes(key_type, key_id, &data)
    }
    fn load(&self, key_type: String, key_id: String) -> Result<Option<Vec<u8>>, MlsStorageError> {
        self.load_bytes(key_type, key_id)
    }
    fn delete(&self, key_type: String, key_id: String) -> Result<(), MlsStorageError> {
        self.remove_key(key_type, key_id)
    }
    fn list_keys(&self, key_type: String) -> Result<Vec<String>, MlsStorageError> {
        self.keys(key_type)
    }
}
