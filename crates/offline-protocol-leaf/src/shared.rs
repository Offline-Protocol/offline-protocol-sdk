//! The store handle, and the one place that decides what it is.
//!
//! # One handle type per target, named in one file
//!
//! A [`LeafDevice`](crate::LeafDevice) and the adapters it builds share one
//! store, so the handle is reference counted. Where the processor has atomic
//! compare-and-swap that is `alloc::sync::Arc`. Where it has none (ESP32-C3,
//! ESP32-C2, every Cortex-M0) `alloc::sync` does not exist, and the handle is
//! `portable_atomic_util::Arc` instead, the same choice the pinned mls-rs makes
//! for its own handles.
//!
//! The failure this file prevents is a build that works on every machine a
//! developer owns and nowhere a device runs: a host always has atomics, so an
//! `alloc::sync` import anywhere else in this crate compiles, passes every
//! test, and breaks only the bare-metal targets. `only_one_file_names_the_counted_pointer`
//! in this module refuses that import on the host, and the `embedded-core` CI
//! job refuses it on the targets.
//!
//! # Building a handle
//!
//! [`shared_store`] is the portable way. The replacement pointer cannot turn
//! an `Arc<MyStore>` into an `Arc<dyn LeafStore>` on stable Rust, so the
//! constructor goes through a `Box<dyn LeafStore>`, which costs one extra
//! allocation and one copy of the store value, once, at boot. It takes that
//! route on every target, so the host tests run the code a device runs.
//!
//! On a target with atomics [`SharedStore`] is `alloc::sync::Arc<dyn LeafStore>`,
//! which is `std::sync::Arc` wherever `std` exists, so code that builds one with
//! `Arc::new` keeps compiling there. On a target without, only [`shared_store`]
//! builds one.
//!
//! # What a target without compare-and-swap owes
//!
//! A critical-section implementation, linked by the firmware. The reference
//! count is only as sound as it is: an implementation that does not actually
//! exclude interrupts or other cores makes the count racy, and a racy count is
//! a use-after-free of the store.

use alloc::boxed::Box;

#[cfg(target_has_atomic = "ptr")]
pub(crate) use alloc::sync::Arc;
#[cfg(not(target_has_atomic = "ptr"))]
pub(crate) use portable_atomic_util::Arc;

use crate::store::LeafStore;

/// A shared handle to the device's store.
///
/// On a target with atomic compare-and-swap this is `alloc::sync::Arc`, the
/// same type as `std::sync::Arc`. On one without, it is
/// `portable_atomic_util::Arc`. Build one with [`shared_store`] to write code
/// that compiles on both.
pub type SharedStore = Arc<dyn LeafStore>;

/// Wraps a store in the handle a [`LeafDevice`](crate::LeafDevice) takes.
///
/// ```
/// use offline_protocol_leaf::{shared_store, LeafDevice, MemoryStore};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let device = LeafDevice::open(shared_store(MemoryStore::new()), "com.example.lock")?;
/// # let _ = device;
/// # Ok(())
/// # }
/// ```
pub fn shared_store<S: LeafStore + 'static>(store: S) -> SharedStore {
    let boxed: Box<dyn LeafStore> = Box::new(store);
    Arc::from(boxed)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, KEY_TYPE_IDENTITY};
    use crate::LeafDevice;
    use std::string::String;
    use std::vec::Vec;
    use std::{format, fs, path::PathBuf};

    #[test]
    fn a_store_built_by_the_constructor_is_the_one_the_device_writes() {
        let store = shared_store(MemoryStore::new());
        let reader = Arc::clone(&store);
        let _device = LeafDevice::open(store, "com.example.lock").expect("device opens");

        let identity = reader
            .load(KEY_TYPE_IDENTITY, "signature_public")
            .expect("load succeeds");
        assert!(
            identity.is_some(),
            "provisioning must write through the handle the constructor built"
        );
    }

    /// The import shapes that name `alloc::sync` or its pointer, including the
    /// ones rustfmt leaves on one line (`sync::Arc}`) or nests
    /// (`sync::{Arc, Weak}`). `std::sync::Mutex` in the `std`-only test store
    /// is legitimate and matches none of them.
    fn names_the_counted_pointer(line: &str) -> bool {
        let line = line.trim_start();
        !line.starts_with("//")
            && ["alloc::sync", "sync::Arc", "sync::Weak", "sync::{"]
                .iter()
                .any(|pattern| line.contains(pattern))
    }

    fn rust_files(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
        for path in fs::read_dir(dir)
            .expect("src is readable")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
        {
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    #[test]
    fn the_guard_sees_every_import_shape() {
        for shape in [
            "use alloc::sync::Arc;",
            "    sync::Arc,",
            "use alloc::{string::String, sync::Arc};",
            "use alloc::{sync::{Arc, Weak}, vec::Vec};",
            "let store: alloc::sync::Arc<dyn LeafStore>;",
        ] {
            assert!(names_the_counted_pointer(shape), "missed: {shape}");
        }
        for fine in [
            "use std::sync::Mutex;",
            "use crate::shared::Arc;",
            "// alloc::sync is not available here",
        ] {
            assert!(!names_the_counted_pointer(fine), "false alarm: {fine}");
        }
    }

    #[test]
    fn only_one_file_names_the_counted_pointer() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        assert!(!files.is_empty(), "the guard found no source to read");

        let offenders: Vec<String> = files
            .iter()
            .filter(|path| path.file_name().is_some_and(|name| name != "shared.rs"))
            .filter_map(|path| {
                let text = fs::read_to_string(path).ok()?;
                text.lines()
                    .any(names_the_counted_pointer)
                    .then(|| format!("{}", path.display()))
            })
            .collect();

        assert!(
            offenders.is_empty(),
            "these files name alloc::sync directly, which compiles on every host and \
             breaks every target without compare-and-swap; take `Arc` from \
             `crate::shared` instead: {offenders:?}"
        );
    }
}
