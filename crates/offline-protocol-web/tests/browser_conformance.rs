//! Both storage conformance suites, and a reopen, against the log store over
//! real OPFS files, in a dedicated worker of a real browser.
//!
//! The host tests run the same store over memory files, which cannot show
//! what only the browser decides: whether a sync access handle reads back
//! what it was given at an offset, whether `truncate` and `flush` do what the
//! store assumes, and whether a handle closed and opened again sees every
//! acknowledged write. A storage adapter is a supported extension point and
//! is verified (C11); this is that verification for the browser's.
//!
//! Run with `bash bindings/web/scripts/test-conformance.sh`, which names the
//! WebDriver for each browser. A host build compiles this file to nothing.

#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use offline_protocol::mls::MlsStorage;
use offline_protocol_web::log_store::{Category, LogStore, STORE_KEY_BYTES};
use offline_protocol_web::opfs::{OpfsFile, SyncAccessHandle};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemRemoveOptions, FileSystemSyncAccessHandle,
    WorkerGlobalScope,
};

// OPFS sync access handles exist only in a dedicated worker (W1).
wasm_bindgen_test_configure!(run_in_dedicated_worker);

const KEY: [u8; STORE_KEY_BYTES] = [7; STORE_KEY_BYTES];

/// A directory of its own under the origin's private root, removed on drop
/// of nothing: [`Scratch::remove`] is awaited by each test, since removal is
/// asynchronous.
struct Scratch {
    root: FileSystemDirectoryHandle,
    name: String,
    directory: FileSystemDirectoryHandle,
}

impl Scratch {
    async fn new() -> Self {
        let scope: WorkerGlobalScope = js_sys::global().unchecked_into();
        let root: FileSystemDirectoryHandle =
            resolve(scope.navigator().storage().get_directory()).await;
        let name = format!("conformance-{}", js_sys::Math::random().to_bits());
        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(true);
        let directory = resolve(root.get_directory_handle_with_options(&name, &options)).await;
        Self {
            root,
            name,
            directory,
        }
    }

    /// Opens the two log files as the worker does, returning the handles to
    /// close later and the files the store takes.
    async fn open(&self) -> (Vec<FileSystemSyncAccessHandle>, OpfsFile, OpfsFile) {
        let options = FileSystemGetFileOptions::new();
        options.set_create(true);
        let mut handles = Vec::new();
        for name in ["log-0", "log-1"] {
            let file: FileSystemFileHandle =
                resolve(self.directory.get_file_handle_with_options(name, &options)).await;
            handles.push(
                resolve::<FileSystemSyncAccessHandle>(file.create_sync_access_handle()).await,
            );
        }
        let file = |handle: &FileSystemSyncAccessHandle| {
            OpfsFile::new(JsValue::from(handle.clone()).unchecked_into::<SyncAccessHandle>())
        };
        let (first, second) = (file(&handles[0]), file(&handles[1]));
        (handles, first, second)
    }

    async fn remove(self) {
        let options = FileSystemRemoveOptions::new();
        options.set_recursive(true);
        let _: JsValue = resolve(self.root.remove_entry_with_options(&self.name, &options)).await;
    }
}

async fn resolve<T: JsCast>(promise: js_sys::Promise) -> T {
    JsFuture::from(promise)
        .await
        .expect("an OPFS call failed")
        .unchecked_into()
}

fn close(handles: Vec<FileSystemSyncAccessHandle>) {
    for handle in handles {
        handle.close();
    }
}

#[wasm_bindgen_test]
async fn the_opfs_log_store_passes_the_mls_suite() {
    let scratch = Scratch::new().await;
    let (handles, first, second) = scratch.open().await;
    let store = LogStore::open(&KEY, Category::Mls, first, second).expect("open");
    let report = offline_protocol::mls_storage_conformance::run(&store);
    drop(store);
    close(handles);
    scratch.remove().await;
    assert!(report.is_green(), "{}", report.summary());
}

#[wasm_bindgen_test]
async fn the_opfs_log_store_passes_the_protocol_state_suite() {
    let scratch = Scratch::new().await;
    let (handles, first, second) = scratch.open().await;
    let store = LogStore::open(&KEY, Category::ProtocolState, first, second).expect("open");
    let report = offline_protocol::storage_conformance::run(&store);
    drop(store);
    close(handles);
    scratch.remove().await;
    assert!(report.is_green(), "{}", report.summary());
}

/// The suites run against one open store, so they cannot see a write that was
/// acknowledged and never reached the file. Enough overwrites to compact
/// more than once also put `truncate` and the generation switch on real
/// files.
#[wasm_bindgen_test]
async fn a_reopened_opfs_log_holds_every_acknowledged_write_through_compaction() {
    let scratch = Scratch::new().await;
    let (handles, first, second) = scratch.open().await;
    let store = LogStore::open(&KEY, Category::Mls, first, second).expect("open");
    let value = |round: usize| vec![round as u8; 16 * 1024];
    // 16 KiB a write and a 1 MiB compaction floor: about four compactions.
    for round in 0..256 {
        MlsStorage::store(&store, "kind", "hot", &value(round)).expect("store");
    }
    MlsStorage::store(&store, "kind", "kept", b"kept").expect("store");
    MlsStorage::store(&store, "kind", "gone", b"gone").expect("store");
    MlsStorage::delete(&store, "kind", "gone").expect("delete");
    let compactions = store.stats().compactions;
    drop(store);
    close(handles);

    let (handles, first, second) = scratch.open().await;
    let store = LogStore::open(&KEY, Category::Mls, first, second).expect("reopen");
    let hot = MlsStorage::load(&store, "kind", "hot").expect("load");
    let kept = MlsStorage::load(&store, "kind", "kept").expect("load");
    let gone = MlsStorage::load(&store, "kind", "gone").expect("load");
    drop(store);
    close(handles);
    scratch.remove().await;
    assert_eq!(hot, Some(value(255)));
    assert_eq!(kept.as_deref(), Some(&b"kept"[..]));
    assert_eq!(gone, None);
    assert!(
        compactions >= 2,
        "the writes compacted {compactions} times, so the reopen proved less than it claims"
    );
}
