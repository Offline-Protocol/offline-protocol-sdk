//! The log store's files in the browser: OPFS synchronous access handles.
//!
//! The worker opens the handles (opening is asynchronous) and hands them in
//! before the engine initializes. A handle is a JavaScript object, which Rust
//! cannot hold across the storage traits' `Send + Sync` bound, so the handles
//! live in a table on the worker's one thread and [`OpfsFile`] names a slot.
//! A worker has exactly one thread, so the table is never reached from
//! another.

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

use crate::log_store::LogFile;

#[wasm_bindgen]
extern "C" {
    /// An OPFS `FileSystemSyncAccessHandle`, opened by the worker.
    #[wasm_bindgen(typescript_type = "FileSystemSyncAccessHandle")]
    pub type SyncAccessHandle;

    #[wasm_bindgen(method, catch, js_name = getSize)]
    fn get_size(this: &SyncAccessHandle) -> Result<f64, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn read(this: &SyncAccessHandle, buffer: &mut [u8], options: &JsValue) -> Result<f64, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn write(this: &SyncAccessHandle, buffer: &[u8], options: &JsValue) -> Result<f64, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn truncate(this: &SyncAccessHandle, size: f64) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    fn flush(this: &SyncAccessHandle) -> Result<(), JsValue>;
}

thread_local! {
    static HANDLES: RefCell<Vec<SyncAccessHandle>> = const { RefCell::new(Vec::new()) };
}

/// One handle in the worker's table.
#[derive(Debug)]
pub struct OpfsFile {
    slot: usize,
}

impl OpfsFile {
    /// Takes `handle` into the table.
    pub fn new(handle: SyncAccessHandle) -> Self {
        let slot = HANDLES.with(|handles| {
            let mut handles = handles.borrow_mut();
            handles.push(handle);
            handles.len() - 1
        });
        Self { slot }
    }

    fn with<T>(
        &self,
        op: impl FnOnce(&SyncAccessHandle) -> Result<T, JsValue>,
    ) -> Result<T, String> {
        HANDLES.with(|handles| {
            let handles = handles.borrow();
            let handle = handles
                .get(self.slot)
                .ok_or_else(|| "the file handle is gone".to_string())?;
            op(handle).map_err(describe)
        })
    }
}

fn describe(error: JsValue) -> String {
    js_sys::Error::from(error)
        .message()
        .as_string()
        .unwrap_or_else(|| "an OPFS call failed".to_string())
}

fn at(offset: u64) -> Result<JsValue, JsValue> {
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &"at".into(), &JsValue::from_f64(offset as f64))?;
    Ok(options.into())
}

impl LogFile for OpfsFile {
    fn size(&mut self) -> Result<u64, String> {
        self.with(|handle| handle.get_size())
            .map(|size| size as u64)
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), String> {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = self
                .with(|handle| handle.read(&mut buffer[filled..], &at(offset + filled as u64)?))?
                as usize;
            if read == 0 {
                return Err("short read".to_string());
            }
            filled += read;
        }
        Ok(())
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> Result<(), String> {
        let mut written = 0;
        while written < data.len() {
            let wrote = self
                .with(|handle| handle.write(&data[written..], &at(offset + written as u64)?))?
                as usize;
            if wrote == 0 {
                return Err("short write".to_string());
            }
            written += wrote;
        }
        Ok(())
    }

    fn truncate(&mut self, len: u64) -> Result<(), String> {
        self.with(|handle| handle.truncate(len as f64))
    }

    fn flush(&mut self) -> Result<(), String> {
        self.with(|handle| handle.flush())
    }
}
