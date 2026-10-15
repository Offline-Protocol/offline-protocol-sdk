//! The `extern "C"` functions, called as C calls them, and the guards that
//! hold the boundary's files and documents together.

use core::ffi::{c_char, c_void};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::adapters::{self, OfflineProtocolStorage, OfflineProtocolStorageStatus};
use super::ffi::*;
use super::tests::minimal_config;
use crate::{MlsStorageError, MlsStorageProvider};

fn c(text: &str) -> CString {
    CString::new(text).expect("no NUL")
}

fn zeroed_error() -> OfflineProtocolError {
    OfflineProtocolError {
        status: OfflineProtocolStatus::Ok,
        code: -1,
        variant: std::ptr::null_mut(),
        message: std::ptr::null_mut(),
    }
}

/// Reads and frees a string the library handed out.
fn take(s: *mut c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: a string this library returned.
    let text = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    // SAFETY: freed once.
    unsafe { offline_protocol_string_free(s) };
    Some(text)
}

fn error_text(err: &mut OfflineProtocolError) -> (Option<String>, Option<String>) {
    let read = |p: *mut c_char| {
        // SAFETY: the library's own string, still owned by `err`.
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    };
    let out = (read(err.variant), read(err.message));
    // SAFETY: an error this library wrote.
    unsafe { offline_protocol_error_clear(err) };
    out
}

/// An engine created through the C entry point.
struct Handle(*mut OfflineProtocolHandle);

impl Handle {
    fn new() -> Self {
        let config = c(&minimal_config().to_string());
        let mut handle = std::ptr::null_mut();
        let mut err = zeroed_error();
        // SAFETY: valid pointers.
        let status = unsafe { offline_protocol_create(config.as_ptr(), &mut handle, &mut err) };
        assert_eq!(
            status,
            OfflineProtocolStatus::Ok,
            "{:?}",
            error_text(&mut err)
        );
        Self(handle)
    }

    fn call(
        &self,
        method: &str,
        params: Option<&str>,
    ) -> Result<String, (OfflineProtocolStatus, OfflineProtocolError)> {
        let method = c(method);
        let params = params.map(c);
        let mut out = std::ptr::null_mut();
        let mut err = zeroed_error();
        // SAFETY: valid pointers; `params` outlives the call.
        let status = unsafe {
            offline_protocol_call(
                self.0,
                method.as_ptr(),
                params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                &mut out,
                &mut err,
            )
        };
        if status == OfflineProtocolStatus::Ok {
            Ok(take(out).expect("a result"))
        } else {
            Err((status, err))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle from create, freed once.
        unsafe { offline_protocol_free(self.0) };
    }
}

#[test]
fn the_version_is_the_workspace_version() {
    // SAFETY: a static string.
    let version = unsafe { CStr::from_ptr(offline_protocol_version()) };
    assert_eq!(version.to_str(), Ok(env!("CARGO_PKG_VERSION")));
}

#[test]
fn null_pointers_are_refused_with_a_reason() {
    let mut err = zeroed_error();
    let mut out = std::ptr::null_mut();
    let method = c("get_state");
    // SAFETY: a null handle is what is under test.
    let status = unsafe {
        offline_protocol_call(
            std::ptr::null(),
            method.as_ptr(),
            std::ptr::null(),
            &mut out,
            &mut err,
        )
    };
    assert_eq!(status, OfflineProtocolStatus::InvalidArgument);
    assert_eq!(err.status, status);
    assert_eq!(err.code, -1);
    let (variant, message) = error_text(&mut err);
    assert_eq!(variant, None);
    assert_eq!(message.as_deref(), Some("the handle is null"));
    assert!(err.message.is_null(), "error_clear resets the struct");

    let handle = Handle::new();
    // SAFETY: a null method is what is under test.
    let status = unsafe {
        offline_protocol_call(
            handle.0,
            std::ptr::null(),
            std::ptr::null(),
            &mut out,
            &mut err,
        )
    };
    assert_eq!(status, OfflineProtocolStatus::InvalidArgument);
    // A null error pointer is allowed: the status alone is the answer.
    // SAFETY: as above.
    let status = unsafe {
        offline_protocol_call(
            handle.0,
            method.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(
        status,
        OfflineProtocolStatus::InvalidArgument,
        "a null out_result"
    );
    // SAFETY: freeing null is allowed.
    unsafe {
        offline_protocol_free(std::ptr::null_mut());
        offline_protocol_string_free(std::ptr::null_mut());
        offline_protocol_error_clear(std::ptr::null_mut());
    }
    assert!(unsafe { offline_protocol_poll_event(std::ptr::null()) }.is_null());
}

#[test]
fn text_that_is_not_utf8_is_refused() {
    let handle = Handle::new();
    let bad = CString::new(vec![0xff, 0xfe]).expect("no NUL");
    let mut out = std::ptr::null_mut();
    let mut err = zeroed_error();
    // SAFETY: valid pointers; the bytes are the point.
    let status = unsafe {
        offline_protocol_call(handle.0, bad.as_ptr(), std::ptr::null(), &mut out, &mut err)
    };
    assert_eq!(status, OfflineProtocolStatus::InvalidArgument);
    assert_eq!(
        error_text(&mut err).1.as_deref(),
        Some("method is not UTF-8")
    );
}

#[test]
fn every_status_reaches_c() {
    let handle = Handle::new();
    assert_eq!(
        handle.call("get_state", None).ok().as_deref(),
        Some("\"Stopped\"")
    );

    let (status, mut err) = handle.call("no_such_method", None).expect_err("unknown");
    assert_eq!(status, OfflineProtocolStatus::MethodNotFound);
    error_text(&mut err);

    let (status, mut err) = handle.call("get_state", Some("{")).expect_err("not JSON");
    assert_eq!(status, OfflineProtocolStatus::InvalidArgument);
    error_text(&mut err);

    let (status, mut err) = handle
        .call("get_state", Some(r#"{"x":1}"#))
        .expect_err("extra");
    assert_eq!(status, OfflineProtocolStatus::InvalidParams);
    error_text(&mut err);

    handle.call("start", None).expect("starts");
    let (status, mut err) = handle
        .call(
            "send_message",
            Some(r#"{"recipient":"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s","content":"hi","priority":"Medium"}"#),
        )
        .expect_err("no storage yet");
    assert_eq!(status, OfflineProtocolStatus::EngineError);
    let code = err.code;
    let (variant, message) = error_text(&mut err);
    // Encryption is required by default and nothing is attached yet. The
    // local API's table gives EncryptFailed as -32006, so position 6.
    assert_eq!(variant.as_deref(), Some("EncryptFailed"));
    assert_eq!(code, 6);
    assert!(message.is_some_and(|m| m.contains("initialize_mls")));
    handle.call("stop", None).expect("stops");
}

#[test]
fn a_panic_is_answered_as_internal_rather_than_aborting() {
    let mut err = zeroed_error();
    // SAFETY: `err` is valid.
    let status = unsafe { super::ffi::boundary_for_test(&mut err, || panic!("boom")) };
    assert_eq!(status, OfflineProtocolStatus::Internal);
    let message = error_text(&mut err).1.expect("a reason");
    assert!(message.contains("boom"), "{message}");
}

#[derive(Default)]
struct Seen {
    events: Mutex<Vec<String>>,
    thread: Mutex<Option<std::thread::ThreadId>>,
    drops: AtomicUsize,
}

unsafe extern "C" fn record_event(ctx: *mut c_void, json: *const c_char) {
    // SAFETY: `ctx` is a `Seen` the test keeps alive; `json` lives for the call.
    let seen = unsafe { &*ctx.cast::<Seen>() };
    let text = unsafe { CStr::from_ptr(json) }
        .to_string_lossy()
        .into_owned();
    seen.events.lock().expect("lock").push(text);
    *seen.thread.lock().expect("lock") = Some(std::thread::current().id());
}

unsafe extern "C" fn count_drop(ctx: *mut c_void) {
    // SAFETY: as above.
    let seen = unsafe { &*ctx.cast::<Seen>() };
    seen.drops.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn events_arrive_on_the_calling_thread_and_in_the_poll_queue() {
    let handle = Handle::new();
    let seen = Box::new(Seen::default());
    let ctx = std::ptr::from_ref::<Seen>(&seen)
        .cast_mut()
        .cast::<c_void>();
    let mut err = zeroed_error();
    // SAFETY: valid handle; `seen` outlives the handle.
    let status = unsafe {
        offline_protocol_set_event_callback(
            handle.0,
            Some(record_event),
            Some(count_drop),
            ctx,
            &mut err,
        )
    };
    assert_eq!(status, OfflineProtocolStatus::Ok);

    handle.call("emit_test_event", None).expect("emits");
    let events = seen.events.lock().expect("lock").clone();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        *seen.thread.lock().expect("lock"),
        Some(std::thread::current().id())
    );

    // SAFETY: valid handle.
    let polled = take(unsafe { offline_protocol_poll_event(handle.0) });
    assert_eq!(
        polled.as_deref(),
        Some(events[0].as_str()),
        "the same text on both paths"
    );
    assert!(take(unsafe { offline_protocol_poll_event(handle.0) }).is_none());

    // Replacing the callback releases the first context exactly once.
    // SAFETY: as above; a null function stops delivery.
    let status = unsafe {
        offline_protocol_set_event_callback(handle.0, None, None, std::ptr::null_mut(), &mut err)
    };
    assert_eq!(status, OfflineProtocolStatus::Ok);
    assert_eq!(seen.drops.load(Ordering::SeqCst), 1);
    handle.call("emit_test_event", None).expect("emits");
    assert_eq!(
        seen.events.lock().expect("lock").len(),
        1,
        "delivery stopped"
    );
    drop(handle);
    assert_eq!(
        seen.drops.load(Ordering::SeqCst),
        1,
        "dropped once, not again at free"
    );
}

#[test]
fn a_refused_transport_registration_still_releases_its_context() {
    let handle = Handle::new();
    let seen = Box::new(Seen::default());
    let ctx = std::ptr::from_ref::<Seen>(&seen)
        .cast_mut()
        .cast::<c_void>();
    let mut err = zeroed_error();
    // SAFETY: valid handle; 7 is not a transport.
    let status = unsafe {
        offline_protocol_set_transport_callback(handle.0, 7, None, Some(count_drop), ctx, &mut err)
    };
    assert_eq!(status, OfflineProtocolStatus::InvalidArgument);
    error_text(&mut err);
    assert_eq!(seen.drops.load(Ordering::SeqCst), 1);

    // SAFETY: as above, with a real transport.
    let status = unsafe {
        offline_protocol_set_transport_callback(
            handle.0,
            OfflineProtocolTransport::Ble as i32,
            None,
            Some(count_drop),
            ctx,
            &mut err,
        )
    };
    assert_eq!(status, OfflineProtocolStatus::Ok);
    drop(handle);
    assert_eq!(
        seen.drops.load(Ordering::SeqCst),
        2,
        "released when the engine is freed"
    );
}

/// A host provider written the way a C host writes one, over a map.
#[derive(Default)]
struct HostStore {
    map: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    store_answer: Mutex<Option<i32>>,
    releases: AtomicUsize,
    drops: AtomicUsize,
}

fn host(ctx: *mut c_void) -> &'static HostStore {
    // SAFETY: every test keeps its `HostStore` alive past the provider.
    unsafe { &*ctx.cast::<HostStore>() }
}

fn s(p: *const c_char) -> String {
    // SAFETY: the library's NUL-terminated key.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Hands a copy of `bytes` to the library through C's allocator shape.
fn hand_back(bytes: &[u8], out: *mut *mut u8, out_len: *mut usize) {
    let boxed: Box<[u8]> = bytes.into();
    // SAFETY: the library's out pointers.
    unsafe {
        *out_len = boxed.len();
        *out = Box::into_raw(boxed).cast::<u8>();
    }
}

unsafe extern "C" fn h_store(
    ctx: *mut c_void,
    t: *const c_char,
    i: *const c_char,
    d: *const u8,
    n: usize,
) -> i32 {
    let host = host(ctx);
    if let Some(answer) = *host.store_answer.lock().expect("lock") {
        return answer;
    }
    // SAFETY: `n` readable bytes at `d`.
    let data = unsafe { std::slice::from_raw_parts(d, n) }.to_vec();
    host.map.lock().expect("lock").insert((s(t), s(i)), data);
    0
}

unsafe extern "C" fn h_load(
    ctx: *mut c_void,
    t: *const c_char,
    i: *const c_char,
    out: *mut *mut u8,
    n: *mut usize,
) -> i32 {
    if let Some(v) = host(ctx).map.lock().expect("lock").get(&(s(t), s(i))) {
        hand_back(v, out, n);
    }
    0
}

unsafe extern "C" fn h_remove(ctx: *mut c_void, t: *const c_char, i: *const c_char) -> i32 {
    host(ctx).map.lock().expect("lock").remove(&(s(t), s(i)));
    0
}

unsafe extern "C" fn h_list(
    ctx: *mut c_void,
    t: *const c_char,
    out: *mut *mut u8,
    n: *mut usize,
) -> i32 {
    let t = s(t);
    let keys: Vec<String> = host(ctx)
        .map
        .lock()
        .expect("lock")
        .keys()
        .filter(|(kt, _)| *kt == t)
        .map(|(_, id)| id.clone())
        .collect();
    hand_back(
        serde_json::to_string(&keys).expect("json").as_bytes(),
        out,
        n,
    );
    0
}

unsafe extern "C" fn h_release(ctx: *mut c_void, data: *mut u8, len: usize) {
    host(ctx).releases.fetch_add(1, Ordering::SeqCst);
    // SAFETY: a buffer `hand_back` made with exactly this length.
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) });
}

unsafe extern "C" fn h_drop(ctx: *mut c_void) {
    host(ctx).drops.fetch_add(1, Ordering::SeqCst);
}

/// A `load` that fails and hands back a buffer anyway.
unsafe extern "C" fn h_load_fails_with_a_buffer(
    _ctx: *mut c_void,
    _t: *const c_char,
    _i: *const c_char,
    out: *mut *mut u8,
    n: *mut usize,
) -> i32 {
    hand_back(b"partial", out, n);
    OfflineProtocolStorageStatus::LoadFailed as i32
}

/// A `list_keys` that succeeds without writing a buffer.
unsafe extern "C" fn h_list_nothing(
    _ctx: *mut c_void,
    _t: *const c_char,
    _out: *mut *mut u8,
    _n: *mut usize,
) -> i32 {
    OfflineProtocolStorageStatus::Ok as i32
}

fn provider(host: &HostStore) -> OfflineProtocolStorage {
    OfflineProtocolStorage {
        ctx: std::ptr::from_ref(host).cast_mut().cast(),
        store: Some(h_store),
        load: Some(h_load),
        remove: Some(h_remove),
        list_keys: Some(h_list),
        release: Some(h_release),
        drop: None,
    }
}

#[test]
fn a_c_storage_provider_passes_the_conformance_suite() {
    let store = Box::leak(Box::new(HostStore::default()));
    let vt = provider(store);
    let mut out = std::ptr::null_mut();
    let mut err = zeroed_error();
    // SAFETY: a valid provider.
    let status = unsafe { offline_protocol_run_storage_conformance(&vt, &mut out, &mut err) };
    assert_eq!(status, OfflineProtocolStatus::Ok);
    let report: serde_json::Value =
        serde_json::from_str(&take(out).expect("report")).expect("json");
    assert_eq!(report["failures"], serde_json::json!([]), "{report}");
    assert!(report["passed"].as_array().is_some_and(|p| !p.is_empty()));
}

#[test]
fn initialize_mls_over_c_providers_gives_the_engine_an_address() {
    let secure = Box::leak(Box::new(HostStore::default()));
    let state = Box::leak(Box::new(HostStore::default()));
    let handle = Handle::new();
    let mut err = zeroed_error();
    // SAFETY: valid handle and providers.
    let status = unsafe {
        offline_protocol_initialize_mls(handle.0, &provider(secure), &provider(state), &mut err)
    };
    assert_eq!(
        status,
        OfflineProtocolStatus::Ok,
        "{:?}",
        error_text(&mut err)
    );
    let address: Option<String> =
        serde_json::from_str(&handle.call("local_address", None).expect("answers")).expect("json");
    assert!(address.is_some_and(|a| a.starts_with("off1")));
    assert!(
        !secure.map.lock().expect("lock").is_empty(),
        "key material went to the secure provider"
    );
}

#[test]
fn a_provider_missing_a_function_is_refused_up_front() {
    let store = Box::leak(Box::new(HostStore::default()));
    let mut vt = provider(store);
    vt.list_keys = None;
    let mut out = std::ptr::null_mut();
    let mut err = zeroed_error();
    // SAFETY: a valid, if incomplete, provider.
    let status = unsafe { offline_protocol_run_storage_conformance(&vt, &mut out, &mut err) };
    assert_eq!(status, OfflineProtocolStatus::InvalidArgument);
    assert!(error_text(&mut err)
        .1
        .is_some_and(|m| m.contains("list_keys")));
}

#[test]
fn storage_answers_map_to_the_definitions_variants_and_never_invent_corruption() {
    let store = Box::leak(Box::new(HostStore::default()));
    let adapter = adapters::storage(&provider(store))
        .map_err(|e| e.to_owned())
        .expect("complete");
    let cases = [
        (
            OfflineProtocolStorageStatus::StoreFailed as i32,
            "StoreFailed",
        ),
        (
            OfflineProtocolStorageStatus::LoadFailed as i32,
            "LoadFailed",
        ),
        (
            OfflineProtocolStorageStatus::DeleteFailed as i32,
            "DeleteFailed",
        ),
        (
            OfflineProtocolStorageStatus::KeyNotFound as i32,
            "KeyNotFound",
        ),
        (
            OfflineProtocolStorageStatus::CorruptedData as i32,
            "CorruptedData",
        ),
        // Outside the range: the operation's own failure, not corruption.
        (99, "StoreFailed"),
        (-1, "StoreFailed"),
    ];
    for (code, expected) in cases {
        *store.store_answer.lock().expect("lock") = Some(code);
        let result = MlsStorageProvider::store(&adapter, "t".into(), "i".into(), vec![1]);
        let got = match result {
            Err(MlsStorageError::StoreFailed) => "StoreFailed",
            Err(MlsStorageError::LoadFailed) => "LoadFailed",
            Err(MlsStorageError::DeleteFailed) => "DeleteFailed",
            Err(MlsStorageError::KeyNotFound) => "KeyNotFound",
            Err(MlsStorageError::CorruptedData) => "CorruptedData",
            Ok(()) => "Ok",
        };
        assert_eq!(got, expected, "code {code}");
    }
}

#[test]
fn the_storage_status_values_are_the_definitions_positions_plus_one() {
    let udl = include_str!("../offline_protocol.udl");
    let body = udl
        .split_once("enum MlsStorageError {")
        .expect("declared")
        .1
        .split_once("};")
        .expect("terminated")
        .0;
    let declared: Vec<&str> = body
        .split(',')
        .map(|v| v.trim().trim_matches('"'))
        .filter(|v| !v.is_empty())
        .collect();
    let ours = [
        ("StoreFailed", OfflineProtocolStorageStatus::StoreFailed),
        ("LoadFailed", OfflineProtocolStorageStatus::LoadFailed),
        ("DeleteFailed", OfflineProtocolStorageStatus::DeleteFailed),
        ("KeyNotFound", OfflineProtocolStorageStatus::KeyNotFound),
        ("CorruptedData", OfflineProtocolStorageStatus::CorruptedData),
    ];
    assert_eq!(
        declared.len(),
        ours.len(),
        "a variant was added to the definition: {declared:?}"
    );
    for (name, status) in ours {
        let position = declared.iter().position(|d| *d == name).expect("declared");
        assert_eq!(status as usize, position + 1, "{name}");
    }
}

// ---- guards over files ------------------------------------------------------

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The function names a text declares as `#[no_mangle] ... fn name(`.
fn no_mangle_functions(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "#[no_mangle]" {
            pending = true;
        } else if pending {
            if let Some(name) = line
                .split("fn ")
                .nth(1)
                .and_then(|rest| rest.split('(').next())
            {
                names.push(name.trim().to_owned());
                pending = false;
            }
        }
    }
    names
}

#[test]
fn every_c_entry_point_is_declared_in_the_committed_header() {
    let mut files = Vec::new();
    rust_files(&crate_dir().join("src"), &mut files);
    let mut exported: Vec<String> = files
        .iter()
        .flat_map(|f| no_mangle_functions(&read(f)))
        .collect();
    exported.sort();
    assert!(
        exported.len() >= 12,
        "the guard found too few: {exported:?}"
    );
    assert!(
        exported.iter().all(|n| n.starts_with("offline_protocol_")),
        "every C symbol carries the binding's prefix: {exported:?}"
    );

    let header = read(&crate_dir().join("../../bindings/c/include/offline_protocol.h"));
    // Every `offline_protocol_<identifier>(` in the header is a declaration:
    // the documentation names functions in backticks, never with `(`.
    let mut declared: Vec<String> = header
        .match_indices("offline_protocol_")
        .filter_map(|(at, _)| {
            let before = header[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let name: String = header[at..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            header[at + name.len()..].starts_with('(').then_some(name)
        })
        .collect();
    declared.sort();
    declared.dedup();
    assert_eq!(
        exported, declared,
        "the header and the exported functions disagree; run scripts/generate-c-header.sh"
    );
}

#[test]
fn unsafe_is_confined_to_the_c_boundary() {
    let src = crate_dir().join("src");
    let allowed = ["capi/ffi.rs", "capi/adapters.rs", "capi/ffi_tests.rs"].map(|f| src.join(f));
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        allowed.iter().all(|a| files.contains(a)),
        "the guard did not find the boundary"
    );
    let offenders: Vec<String> = files
        .iter()
        .filter(|f| !allowed.contains(f))
        .filter(|f| {
            read(f).lines().any(|line| {
                let code = line.split("//").next().unwrap_or("");
                code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|word| word == "unsafe")
            })
        })
        .map(|f| f.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "unsafe outside the C boundary; the crate allows unsafe_code only for UniFFI's scaffolding \
         and these files: {offenders:?}"
    );
}

#[test]
fn the_hand_written_declarations_are_the_ones_the_bridge_doc_lists() {
    let doc = read(&crate_dir().join("../../docs/bridges/c.md"));
    let table = doc
        .split_once("<!-- hand-written: begin -->")
        .expect("docs/bridges/c.md has the hand-written table markers")
        .1
        .split_once("<!-- hand-written: end -->")
        .expect("terminated")
        .0;
    let mut listed: Vec<&str> = table
        .lines()
        .filter_map(|line| line.trim().strip_prefix("| `"))
        .filter_map(|rest| rest.split('`').next())
        .collect();
    listed.sort_unstable();
    let mut ours: Vec<&str> = super::generated::HAND_WRITTEN.to_vec();
    ours.sort_unstable();
    assert_eq!(
        listed, ours,
        "docs/bridges/c.md must list every declaration offline_protocol_call does not reach"
    );
}

#[test]
fn a_failed_load_still_releases_the_buffer_it_was_handed() {
    let store = Box::leak(Box::new(HostStore::default()));
    let mut vt = provider(store);
    vt.load = Some(h_load_fails_with_a_buffer);
    let adapter = adapters::storage(&vt)
        .map_err(|e| e.to_owned())
        .expect("complete");
    let result = MlsStorageProvider::load(&adapter, "t".into(), "i".into());
    assert!(
        matches!(result, Err(MlsStorageError::LoadFailed)),
        "{result:?}"
    );
    assert_eq!(
        store.releases.load(Ordering::SeqCst),
        1,
        "the buffer goes back to the host even when the answer is a failure"
    );
}

#[test]
fn list_keys_with_no_buffer_is_an_empty_list() {
    let store = Box::leak(Box::new(HostStore::default()));
    let mut vt = provider(store);
    vt.list_keys = Some(h_list_nothing);
    let adapter = adapters::storage(&vt)
        .map_err(|e| e.to_owned())
        .expect("complete");
    let keys = MlsStorageProvider::list_keys(&adapter, "t".into());
    assert_eq!(keys.ok(), Some(Vec::new()));
}

#[test]
fn initialize_mls_after_start_is_refused_and_releases_both_providers() {
    let secure = Box::leak(Box::new(HostStore::default()));
    let state = Box::leak(Box::new(HostStore::default()));
    let (mut secure_vt, mut state_vt) = (provider(secure), provider(state));
    secure_vt.drop = Some(h_drop);
    state_vt.drop = Some(h_drop);
    let handle = Handle::new();
    handle.call("start", None).expect("starts");
    let mut err = zeroed_error();
    // SAFETY: valid handle and providers.
    let status =
        unsafe { offline_protocol_initialize_mls(handle.0, &secure_vt, &state_vt, &mut err) };
    assert_eq!(status, OfflineProtocolStatus::EngineError);
    let (variant, _) = error_text(&mut err);
    assert_eq!(variant.as_deref(), Some("InvalidState"));
    assert_eq!(secure.drops.load(Ordering::SeqCst), 1, "secure provider");
    assert_eq!(state.drops.load(Ordering::SeqCst), 1, "state provider");
    handle.call("stop", None).expect("stops");
}

/// A callback that blocks inside its call until the test releases it.
#[derive(Default)]
struct Gate {
    /// Whether the callback has entered, and whether the test released it.
    state: Mutex<(bool, bool)>,
    changed: std::sync::Condvar,
    drops: AtomicUsize,
}

unsafe extern "C" fn blocking_event(ctx: *mut c_void, _json: *const c_char) {
    // SAFETY: a `Gate` the test keeps alive past the handle.
    let gate = unsafe { &*ctx.cast::<Gate>() };
    let mut state = gate.state.lock().expect("lock");
    state.0 = true;
    gate.changed.notify_all();
    while !state.1 {
        state = gate.changed.wait(state).expect("wait");
    }
}

unsafe extern "C" fn gate_drop(ctx: *mut c_void) {
    // SAFETY: as above.
    let gate = unsafe { &*ctx.cast::<Gate>() };
    gate.drops.fetch_add(1, Ordering::SeqCst);
}

/// The handle, shared with the threads of one test. The header promises a
/// handle is usable from any thread.
struct Shared(*mut OfflineProtocolHandle);
// SAFETY: the engine is `Send + Sync` (`the_engine_is_shareable_across_threads`).
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

#[test]
fn a_replaced_callback_is_dropped_only_after_its_running_call_returns() {
    let handle = Handle::new();
    let gate = Gate::default();
    let ctx = std::ptr::from_ref(&gate).cast_mut().cast::<c_void>();
    let mut err = zeroed_error();
    // SAFETY: valid handle; `gate` outlives the handle.
    let status = unsafe {
        offline_protocol_set_event_callback(
            handle.0,
            Some(blocking_event),
            Some(gate_drop),
            ctx,
            &mut err,
        )
    };
    assert_eq!(status, OfflineProtocolStatus::Ok);
    let shared = Shared(handle.0);

    std::thread::scope(|scope| {
        let shared = &shared;
        let emitter = scope.spawn(move || {
            let method = c("emit_test_event");
            let mut out = std::ptr::null_mut();
            // SAFETY: a live handle; the out pointer is this thread's.
            let status = unsafe {
                offline_protocol_call(
                    shared.0,
                    method.as_ptr(),
                    std::ptr::null(),
                    &mut out,
                    std::ptr::null_mut(),
                )
            };
            take(out);
            status
        });
        {
            let mut state = gate.state.lock().expect("lock");
            while !state.0 {
                state = gate.changed.wait(state).expect("wait");
            }
        }

        // The callback is now running on the emitter. Replace it from here.
        let replacer = scope.spawn(move || {
            // SAFETY: a live handle; a null function stops delivery.
            unsafe {
                offline_protocol_set_event_callback(
                    shared.0,
                    None,
                    None,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(
            gate.drops.load(Ordering::SeqCst),
            0,
            "dropped while its call was still running"
        );

        gate.state.lock().expect("lock").1 = true;
        gate.changed.notify_all();
        assert_eq!(emitter.join().expect("emitter"), OfflineProtocolStatus::Ok);
        assert_eq!(
            replacer.join().expect("replacer"),
            OfflineProtocolStatus::Ok
        );
    });
    assert_eq!(
        gate.drops.load(Ordering::SeqCst),
        1,
        "dropped once, after the call returned"
    );
    drop(handle);
    assert_eq!(gate.drops.load(Ordering::SeqCst), 1, "not again at free");
}
