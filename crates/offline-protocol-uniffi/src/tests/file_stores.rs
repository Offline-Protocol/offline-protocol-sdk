//! `initialize_mls_with_file_stores`: the built-in stores behind the FFI.
//!
//! The store behaviour itself is pinned in `offline_protocol::file_store`.
//! These tests pin what the entry point adds: the account directory comes
//! from the instance's own config, a refusal lands on the documented variant
//! and leaves MLS uninitialized, and a repeated call opens nothing.

use super::*;

/// A unique directory under the system temp dir, removed on drop.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "op-ffi-{label}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn dir(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const KEY: [u8; 32] = [0x5a; 32];

fn instance(profile: &str) -> OfflineProtocol {
    OfflineProtocol::new(ProtocolConfig {
        profile: profile.to_string(),
        ..create_test_config()
    })
    .expect("protocol")
}

fn entries(dir: &str) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(read) => read
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// The identity lives in the MLS store, so a second instance over the same
/// roots and key is the same device. Both stores land in the account
/// directory the bindings derive, so a directory written here opens there.
#[test]
fn the_identity_survives_a_restart_and_lives_in_the_bindings_namespace() {
    let root = TempRoot::new("restart");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "restart-user");

    let first = instance("restart-user");
    first
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first init");
    let address = first.local_address().expect("an address after init");
    assert!(first.is_mls_initialized());
    assert_eq!(entries(&mls), vec![namespace.clone()]);
    assert_eq!(entries(&state), vec![namespace]);
    drop(first);

    let second = instance("restart-user");
    second
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("reopen");
    assert_eq!(second.local_address(), Some(address));
}

/// A wrong key is the operator's to fix and must say so. It is refused by
/// the MLS store before anything is read, MLS stays uninitialized so no
/// fresh identity is minted, and the protocol-state store is never opened.
#[test]
fn a_wrong_key_is_refused_before_anything_changes() {
    let root = TempRoot::new("wrong-key");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let first = instance("wrong-key-user");
    first
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first init");
    let address = first.local_address().expect("address");
    drop(first);
    std::fs::remove_dir_all(&state).expect("remove state root");

    let second = instance("wrong-key-user");
    let err = second
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), vec![0x33; 32])
        .expect_err("a wrong key must be refused");
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m) if m.contains("store key")),
        "expected InvalidConfiguration naming the key, got {err:?}"
    );
    assert!(!second.is_mls_initialized());
    assert_eq!(second.local_address(), None);
    assert!(
        entries(&state).is_empty(),
        "a refused key must not create the protocol-state directory"
    );

    // The refusal left the store as it was: the right key still opens it.
    second
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("the right key after a refusal");
    assert_eq!(second.local_address(), Some(address));
}

#[test]
fn a_bad_key_or_an_empty_root_is_an_invalid_argument() {
    let root = TempRoot::new("args");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let protocol = instance("args-user");
    for (mls_root, state_root, key, what) in [
        (mls.clone(), state.clone(), vec![0x5a; 31], "a 31-byte key"),
        (mls.clone(), state.clone(), vec![0x5a; 33], "a 33-byte key"),
        (mls.clone(), state.clone(), vec![0u8; 32], "an all-zero key"),
        (
            String::new(),
            state.clone(),
            KEY.to_vec(),
            "an empty mls_root",
        ),
        (
            mls.clone(),
            "  ".to_string(),
            KEY.to_vec(),
            "a blank state_root",
        ),
    ] {
        let err = protocol
            .initialize_mls_with_file_stores(mls_root, state_root, key)
            .expect_err(what);
        assert!(
            matches!(err, ProtocolError::InvalidArgument(_)),
            "{what}: expected InvalidArgument, got {err:?}"
        );
        assert!(!protocol.is_mls_initialized(), "{what}");
    }
    assert!(entries(&mls).is_empty() && entries(&state).is_empty());
}

/// Two instances over one account directory would each advance ratchet
/// state the other never sees. The second is refused as a state problem,
/// not a configuration one: its arguments are fine, the directory is busy.
#[test]
fn a_second_instance_over_the_same_directory_is_refused() {
    let root = TempRoot::new("in-use");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let first = instance("in-use-user");
    first
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first init");

    let second = instance("in-use-user");
    let err = second
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect_err("the directory is held");
    assert!(
        matches!(&err, ProtocolError::InvalidState(m) if m.contains("already open")),
        "expected InvalidState, got {err:?}"
    );
    assert!(!second.is_mls_initialized());
    assert!(first.is_mls_initialized());
}

/// A repeated call opens nothing. Opening would be refused by the store's
/// own lock (this instance holds it), and a wrong key on the repeat is
/// ignored rather than checked: the first call is the one that counted.
#[test]
fn a_repeated_call_is_idempotent_and_opens_nothing() {
    let root = TempRoot::new("repeat");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let protocol = instance("repeat-user");
    protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first init");
    let address = protocol.local_address();
    protocol
        .initialize_mls_with_file_stores(mls, state, vec![0x33; 32])
        .expect("a repeated call is Ok");
    assert_eq!(protocol.local_address(), address);
}

/// Same ordering rule as `initialize_mls`, checked before the stores open,
/// so a refused call leaves no directory behind.
#[test]
fn after_start_it_is_refused_before_the_stores_open() {
    let root = TempRoot::new("late");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let protocol = instance("late-user");
    protocol.start().expect("start");
    let err = protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect_err("after start");
    assert!(matches!(err, ProtocolError::InvalidState(_)), "{err:?}");
    assert!(!protocol.is_mls_initialized());
    assert!(entries(&mls).is_empty() && entries(&state).is_empty());
}
