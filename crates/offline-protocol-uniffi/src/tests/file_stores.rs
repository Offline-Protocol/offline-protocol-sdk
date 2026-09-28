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
    assert!(
        matches!(&err, ProtocolError::InvalidState(m)
            if m.starts_with("initialize_mls_with_file_stores must be called")),
        "the refusal must name the call that was made: {err:?}"
    );
    assert!(!protocol.is_mls_initialized());
    assert!(entries(&mls).is_empty() && entries(&state).is_empty());
}

/// One directory for both roots, or one inside the other, would put the
/// identity inside a directory the installation removes. Refused before
/// anything is created, however the roots are spelled.
#[test]
fn overlapping_roots_are_an_invalid_argument() {
    let root = TempRoot::new("overlap");
    let one = root.dir("one");
    let protocol = instance("overlap-user");
    let refuse = |mls_root: String, state_root: String| {
        let err = protocol
            .initialize_mls_with_file_stores(mls_root.clone(), state_root.clone(), KEY.to_vec())
            .expect_err("overlapping roots");
        assert!(
            matches!(&err, ProtocolError::InvalidArgument(m) if m.contains("different directories")),
            "{mls_root} / {state_root}: expected InvalidArgument, got {err:?}"
        );
    };
    // Neither exists: resolved by components.
    refuse(one.clone(), one.clone());
    refuse(one.clone(), format!("{one}/"));
    refuse(one.clone(), format!("{one}/./"));
    refuse(one.clone(), root.dir("two/../one"));
    refuse(format!("{one}/mls"), one.clone());
    refuse(one.clone(), format!("{one}/state"));
    assert!(entries(&root.dir("")).is_empty(), "nothing may be created");

    // Both exist: resolved by their canonical paths, so a link to the same
    // directory is the same directory.
    std::fs::create_dir_all(&one).expect("one");
    #[cfg(unix)]
    {
        let link = root.dir("link");
        std::os::unix::fs::symlink(&one, &link).expect("symlink");
        refuse(one.clone(), link.clone());
        refuse(format!("{link}/mls"), one.clone());
    }
    refuse(one.clone(), root.dir("one/../one"));
    assert!(!protocol.is_mls_initialized());
    assert!(entries(&one).is_empty());

    // Siblings sharing a name prefix do not overlap.
    protocol
        .initialize_mls_with_file_stores(root.dir("data"), root.dir("data-state"), KEY.to_vec())
        .expect("siblings open");
}

/// A deployment moving onto the file stores while keeping its state root:
/// the MLS store would mint a new identity, and the first restore would
/// delete the old identity's records it cannot unseal. Refused before
/// either store opens, so nothing changes and the remedy is named.
#[test]
fn state_without_an_identity_is_refused_before_anything_changes() {
    let root = TempRoot::new("orphaned-state");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "orphan-user");
    let account = std::path::Path::new(&state).join(&namespace);
    std::fs::create_dir_all(account.join("t_records")).expect("seed state");
    std::fs::write(account.join("t_records").join("k_entry"), b"sealed").expect("seed");

    let protocol = instance("orphan-user");
    let err = protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect_err("state without an identity");
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m)
            if m.contains("fresh directory") && m.contains(&namespace)),
        "expected InvalidConfiguration naming the remedy, got {err:?}"
    );
    assert!(!protocol.is_mls_initialized());
    assert!(entries(&mls).is_empty(), "no identity may be minted");
    assert!(account.join("t_records").join("k_entry").exists());

    // A lock file alone is not state: an earlier open left it, nothing more.
    std::fs::remove_dir_all(account.join("t_records")).expect("clear");
    std::fs::write(account.join("state-store.lock"), b"").expect("lock");
    protocol
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("a lock file alone opens");
    assert!(protocol.local_address().is_some());
}

/// The same state root with the identity that wrote it opens as before.
#[test]
fn state_with_its_identity_reopens() {
    let root = TempRoot::new("paired");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let first = instance("paired-user");
    first
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first");
    let address = first.local_address();
    drop(first);
    let second = instance("paired-user");
    second
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("reopen");
    assert_eq!(second.local_address(), address);
}

/// An attempt refused at the state root has already created the MLS account
/// directory, its lock file and its key check. The guard must still see no
/// identity there, or the corrected retry over the kept state root would
/// mint a new identity and delete the records it cannot unseal.
#[test]
fn a_retry_after_a_failed_state_root_is_still_refused() {
    let root = TempRoot::new("retry-guard");
    let (mls, state, held) = (root.dir("mls"), root.dir("state"), root.dir("held"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "retry-user");
    let account = seed_state(&state, &namespace);

    // The first attempt names a state root another store holds: the guard
    // passes (nothing there), the MLS store opens, the state store refuses.
    let holder = offline_protocol::FileProtocolStateStorage::open(&held, &namespace).expect("hold");
    let protocol = instance("retry-user");
    let err = protocol
        .initialize_mls_with_file_stores(mls.clone(), held, KEY.to_vec())
        .expect_err("a held state root");
    assert!(matches!(err, ProtocolError::InvalidState(_)), "got {err:?}");
    drop(holder);
    assert!(
        !entries(&format!("{mls}/{namespace}")).is_empty(),
        "the refused attempt left the MLS directory, which is the case under test"
    );

    // The operator corrects the state root to the one the service always used.
    let err = protocol
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect_err("kept state with no identity");
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m) if m.contains("fresh directory")),
        "expected the kept-state refusal, got {err:?}"
    );
    assert!(!protocol.is_mls_initialized());
    assert!(account.join("t_records").join("k_entry").exists());
}

/// A directory that cannot be read proves nothing about what it holds, so
/// it is refused rather than taken for empty (a new identity over records)
/// or for absent (a new identity beside an existing one).
#[cfg(unix)]
#[test]
fn an_unreadable_account_directory_is_refused_not_taken_for_empty() {
    use std::os::unix::fs::PermissionsExt;
    let set_mode = |path: &std::path::Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod")
    };
    let root = TempRoot::new("unreadable");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "unreadable-user");
    let account = seed_state(&state, &namespace);
    set_mode(&account, 0o000);
    if std::fs::read_dir(&account).is_ok() {
        // Running as root: permissions do not bind, so there is no case.
        set_mode(&account, 0o700);
        return;
    }
    let protocol = instance("unreadable-user");
    let err = protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect_err("unreadable state account");
    set_mode(&account, 0o700);
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m) if m.contains("unusable")),
        "expected an unusable-directory refusal, got {err:?}"
    );
    assert!(entries(&mls).is_empty(), "no identity may be minted");

    // The identity exists, but its root cannot be read: refused as
    // unreadable, never as "holds no identity".
    std::fs::remove_dir_all(&account).expect("clear");
    protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first open");
    drop(protocol);
    seed_state(&state, &namespace);
    set_mode(std::path::Path::new(&mls), 0o000);
    let result = instance("unreadable-user").initialize_mls_with_file_stores(
        mls.clone(),
        state,
        KEY.to_vec(),
    );
    set_mode(std::path::Path::new(&mls), 0o700);
    let err = result.expect_err("unreadable mls root");
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m)
            if m.contains("unusable") && !m.contains("holds no identity")),
        "expected an unusable-directory refusal, got {err:?}"
    );

    // An account directory that can be listed and not searched: its entries
    // have names and nothing else can be asked of them. That is not "holds
    // no identity" either.
    let mls_account = std::path::Path::new(&mls).join(&namespace);
    for mode in [0o400, 0o600] {
        set_mode(&mls_account, mode);
        let result = instance("unreadable-user").initialize_mls_with_file_stores(
            mls.clone(),
            root.dir("state"),
            KEY.to_vec(),
        );
        set_mode(&mls_account, 0o700);
        let err = result.expect_err("unsearchable mls account directory");
        assert!(
            matches!(&err, ProtocolError::InvalidConfiguration(m)
                if m.contains("unusable") && !m.contains("holds no identity")),
            "mode {mode:o}: expected an unusable-directory refusal, got {err:?}"
        );
    }
}

fn seed_state(state: &str, namespace: &str) -> std::path::PathBuf {
    let account = std::path::Path::new(state).join(namespace);
    std::fs::create_dir_all(account.join("t_records")).expect("seed state");
    std::fs::write(account.join("t_records").join("k_entry"), b"sealed").expect("seed");
    account
}

/// Every file under `dir` with its bytes, for "no record was changed".
fn snapshot(dir: &str) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![std::path::PathBuf::from(dir)];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).expect("read");
                files.push((path, bytes));
            }
        }
    }
    files.sort();
    files
}

/// Runs a service over the two roots that parks one outbound message, and
/// stops it. Returns the service's address. What it leaves in `state` is
/// what a real deployment leaves: a record sealed under the record key its
/// MLS store holds.
fn run_and_park_one(profile: &str, mls: &str, state: &str) -> String {
    let root = TempRoot::new("park-peer");
    let peer = instance("park-peer");
    peer.initialize_mls_with_file_stores(root.dir("mls"), root.dir("state"), KEY.to_vec())
        .expect("peer init");
    let peer_address = peer.local_address().expect("peer address");

    let service = instance(profile);
    service
        .initialize_mls_with_file_stores(mls.to_string(), state.to_string(), KEY.to_vec())
        .expect("service init");
    let address = service.local_address().expect("address");
    service.start().expect("start");
    service
        .send_message(
            peer_address,
            "parked before the move".to_string(),
            MessagePriority::Medium,
            None,
        )
        .expect("send");
    service.stop().expect("stop");
    drop(service);

    let namespace = offline_protocol::account_storage_namespace("test-app", profile);
    let secure = offline_protocol::SealedFileMlsStorage::open(
        mls,
        &namespace,
        &offline_protocol::StaticStoreKey::from_slice(&KEY).expect("key"),
    )
    .expect("reopen the MLS store");
    assert_eq!(
        offline_protocol::FileProtocolStateStorage::open(state, &namespace)
            .expect("reopen the state store")
            .sealed_state(&secure)
            .expect("verdict"),
        offline_protocol::SealedState::Opens,
        "the fixture has to leave sealed state, or the tests below prove nothing"
    );
    address
}

/// The kept-state refusal asks whether the MLS store holds an identity. It
/// cannot see an identity that is not the one that wrote the state: an
/// operator who followed that refusal's advice (a fresh state root) and later
/// puts the old state root back has an identity in place and state it cannot
/// open. The engine would delete that state as unreadable. Refused instead,
/// with every record as it was and both directories free again.
#[test]
fn state_sealed_by_another_identity_is_refused_and_kept() {
    let root = TempRoot::new("foreign-state");
    let (old_mls, old_state) = (root.dir("old-mls"), root.dir("old-state"));
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let old_address = run_and_park_one("foreign-user", &old_mls, &old_state);

    let protocol = instance("foreign-user");
    protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("the new identity over a fresh state root");
    let address = protocol.local_address().expect("address");
    assert_ne!(address, old_address);
    drop(protocol);

    let before = snapshot(&old_state);
    let identity_before = snapshot(&mls);
    let protocol = instance("foreign-user");
    let err = protocol
        .initialize_mls_with_file_stores(mls.clone(), old_state.clone(), KEY.to_vec())
        .expect_err("state another identity sealed");
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m)
            if m.contains("another identity") && m.contains("fresh directory")),
        "expected the foreign-state refusal, got {err:?}"
    );
    assert!(!protocol.is_mls_initialized());
    assert_eq!(snapshot(&old_state), before, "no record may be changed");
    assert_eq!(snapshot(&mls), identity_before);

    // The refusal released both directories: the same instance opens the
    // pair that belongs together, and the old pair still opens as itself.
    protocol
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("the identity with its own state");
    assert_eq!(protocol.local_address(), Some(address));
    let old = instance("foreign-user");
    old.initialize_mls_with_file_stores(old_mls, old_state, KEY.to_vec())
        .expect("the old pair");
    assert_eq!(old.local_address(), Some(old_address));
}

/// A first write that failed (a full disk, a crash) leaves a type directory
/// with no record in it. That is not an identity, and taking it for one
/// would let kept state through to be deleted.
#[test]
fn a_type_directory_with_no_record_is_not_an_identity() {
    let root = TempRoot::new("empty-type");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "empty-type-user");
    let account = seed_state(&state, &namespace);
    let type_directory = std::path::Path::new(&mls)
        .join(&namespace)
        .join(format!("c_{}", "0".repeat(64)));
    std::fs::create_dir_all(&type_directory).expect("type directory");
    std::fs::write(type_directory.join(".write-crashed"), b"half").expect("temporary");

    let protocol = instance("empty-type-user");
    let err = protocol
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect_err("kept state with no identity");
    assert!(
        matches!(&err, ProtocolError::InvalidConfiguration(m) if m.contains("fresh directory")),
        "expected the kept-state refusal, got {err:?}"
    );
    assert!(!protocol.is_mls_initialized());
    assert!(account.join("t_records").join("k_entry").exists());
}

/// Two spellings that do not compare equal can still be one directory: a
/// volume that folds case makes `one` and `ONE` the same place, and nothing
/// can know that before either exists. Asked again once both do. On a volume
/// that keeps them apart, they are two roots and open as two.
#[test]
fn roots_that_are_one_directory_on_this_volume_are_refused() {
    let root = TempRoot::new("folded");
    std::fs::create_dir(root.dir("probe")).expect("probe");
    let folds_case = std::path::Path::new(&root.dir("PROBE")).exists();
    std::fs::remove_dir(root.dir("probe")).expect("remove probe");

    let (lower, upper) = (root.dir("one"), root.dir("ONE"));
    let protocol = instance("folded-user");
    let outcome = protocol.initialize_mls_with_file_stores(lower, upper, KEY.to_vec());
    if !folds_case {
        outcome.expect("two directories on a volume that keeps case");
        assert_eq!(entries(&root.dir("")).len(), 2);
        return;
    }
    let err = outcome.expect_err("one directory under two spellings");
    assert!(
        matches!(&err, ProtocolError::InvalidArgument(m)
            if m.contains("different directories") && m.contains("on this volume")),
        "expected InvalidArgument, got {err:?}"
    );
    assert!(!protocol.is_mls_initialized());
    let namespace = offline_protocol::account_storage_namespace("test-app", "folded-user");
    assert!(
        !offline_protocol::SealedFileMlsStorage::holds_records(root.dir("one"), &namespace)
            .expect("probe"),
        "no identity may be minted in a directory the installation removes"
    );

    // Nesting is found the same way, through a parent spelled two ways.
    let err = protocol
        .initialize_mls_with_file_stores(root.dir("nest/mls"), root.dir("NEST"), KEY.to_vec())
        .expect_err("one root inside the other under two spellings");
    assert!(
        matches!(&err, ProtocolError::InvalidArgument(m) if m.contains("on this volume")),
        "expected InvalidArgument, got {err:?}"
    );

    // What the refusal left in the shared directory (two lock files and the
    // MLS key check) is not protocol state: a corrected pair that still
    // names that directory as its state root is not told it holds any.
    assert!(!offline_protocol::FileProtocolStateStorage::holds_records(
        root.dir("ONE"),
        &namespace
    )
    .expect("probe"),);
    let corrected = instance("folded-user");
    corrected
        .initialize_mls_with_file_stores(root.dir("identity"), root.dir("ONE"), KEY.to_vec())
        .expect("a fresh mls_root beside the directory the refusal left");
    drop(corrected);

    // Both directories were released, so the corrected roots open.
    protocol
        .initialize_mls_with_file_stores(root.dir("keys"), root.dir("two"), KEY.to_vec())
        .expect("two directories");
}

/// The point of `close_file_stores`: the directories are free while the
/// instance that opened them still exists. A host runtime frees an object
/// when it decides to, and a second instance must not have to wait for that.
/// The closed instance is refused from then on rather than left to run over
/// stores that fail every write.
#[test]
fn close_file_stores_releases_the_directories_while_the_instance_lives() {
    let root = TempRoot::new("close");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let first = instance("close-user");
    first
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("first init");
    let address = first.local_address().expect("address");
    first.start().expect("start");
    first.stop().expect("stop");

    first.close_file_stores().expect("close");
    first
        .close_file_stores()
        .expect("closing twice is closing once");

    let second = instance("close-user");
    second
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("the directories are free while `first` is still alive");
    assert_eq!(second.local_address(), Some(address));
    second.start().expect("the second instance runs");

    for (entry, err) in [
        ("start", first.start().expect_err("start after close")),
        (
            "initialize_mls_with_file_stores",
            first
                .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
                .expect_err("init after close"),
        ),
    ] {
        assert!(
            matches!(&err, ProtocolError::InvalidState(m)
                if m.starts_with(entry) && m.contains("close_file_stores")),
            "{entry}: expected InvalidState naming the call, got {err:?}"
        );
    }
    assert_eq!(first.get_state(), ProtocolState::Stopped);
    // Anything that would write is an error, never a panic, and never a
    // write into the directory the second instance now holds.
    assert!(first.mls_generate_key_package().is_err());
}

/// A running engine writes to its stores, so they are not released under it.
#[test]
fn close_file_stores_is_refused_until_the_protocol_is_stopped() {
    let root = TempRoot::new("close-running");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let protocol = instance("close-running-user");
    protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("init");
    protocol.start().expect("start");

    let err = protocol.close_file_stores().expect_err("while running");
    assert!(
        matches!(&err, ProtocolError::InvalidState(m)
            if m.starts_with("close_file_stores must be called after stop()")),
        "got {err:?}"
    );
    let other = instance("close-running-user");
    assert!(
        matches!(
            other.initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec()),
            Err(ProtocolError::InvalidState(_))
        ),
        "a refused close releases nothing"
    );

    protocol.stop().expect("stop");
    protocol.close_file_stores().expect("close after stop");
    other
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("released");
}

/// An instance that never opened the file stores has nothing to release, and
/// must stay usable: a binding may call this on every teardown without
/// knowing which stores the application chose.
#[test]
fn close_file_stores_is_a_no_op_without_file_stores() {
    let root = TempRoot::new("close-unused");
    let protocol = instance("close-unused-user");
    protocol.close_file_stores().expect("nothing to close");
    protocol.start().expect("start");
    protocol
        .close_file_stores()
        .expect("still nothing to close, running or not");
    protocol.stop().expect("stop");

    // A refused open leaves the instance as it was, not closed.
    let err = protocol
        .initialize_mls_with_file_stores(root.dir("one"), root.dir("one"), KEY.to_vec())
        .expect_err("overlapping roots");
    assert!(matches!(err, ProtocolError::InvalidArgument(_)));
    protocol.close_file_stores().expect("nothing was opened");
    protocol
        .initialize_mls_with_file_stores(root.dir("mls"), root.dir("state"), KEY.to_vec())
        .expect("usable after a no-op close");
    protocol.start().expect("start");
}

/// The uploader keeps its queue in the protocol-state store, so the pipe is
/// stopped, after its final flush, before that store closes. With the ingest
/// unreachable the final flush has nowhere to send its batch but the queue:
/// closing the store first would refuse that write, and the batch would be
/// gone. A refused close leaves telemetry running, and a closed instance
/// cannot start a new pipe over the store it gave up.
#[test]
fn close_file_stores_ends_telemetry_before_the_stores_go() {
    let (client, _guard) = capture_uploads();
    client.set_status(0);
    let root = TempRoot::new("close-telemetry");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let protocol = instance("close-telemetry-user");
    protocol
        .initialize_mls_with_file_stores(mls, state.clone(), KEY.to_vec())
        .expect("init");
    protocol
        .enable_telemetry(telemetry_config(), AppState::Active)
        .expect("telemetry enables");
    protocol.start().expect("start");

    protocol.close_file_stores().expect_err("while running");
    assert!(
        protocol.telemetry_stats().is_some(),
        "a refused close changes nothing"
    );

    // Something for the final flush to carry. The configured interval is an
    // hour, so what is collected from here on is still in the buffer when
    // the close begins.
    let peer = {
        let root = TempRoot::new("close-telemetry-peer");
        let peer = instance("close-telemetry-peer");
        peer.initialize_mls_with_file_stores(root.dir("mls"), root.dir("state"), KEY.to_vec())
            .expect("peer init");
        peer.local_address().expect("peer address")
    };
    protocol
        .send_message(peer, "counted".to_string(), MessagePriority::Medium, None)
        .expect("send");
    protocol.process().expect("process");
    let buffered = protocol.telemetry_stats().expect("enabled").buffered;
    assert!(
        buffered > 0,
        "the fixture has to leave events in the buffer"
    );

    protocol.stop().expect("stop");
    protocol.close_file_stores().expect("close");
    assert!(protocol.telemetry_stats().is_none());

    let namespace = offline_protocol::account_storage_namespace("test-app", "close-telemetry-user");
    let queued = {
        use offline_protocol::ProtocolStateStorage;
        offline_protocol::FileProtocolStateStorage::open(&state, &namespace)
            .expect("the directory is free")
            .list_keys("telemetry_batch")
            .expect("list")
    };
    assert!(
        !queued.is_empty(),
        "the final flush must land in the queue before the store closes"
    );

    let err = protocol
        .enable_telemetry(telemetry_config(), AppState::Active)
        .expect_err("telemetry on a closed instance");
    assert!(
        matches!(&err, ProtocolError::InvalidState(m)
            if m.starts_with("enable_telemetry") && m.contains("close_file_stores")),
        "got {err:?}"
    );
    assert!(protocol.telemetry_stats().is_none());
    offline_protocol::telemetry::pipe::testing::stop_capturing_uploads();
}

/// The engine can refuse a pair after both stores opened: here the identity
/// record is damaged, which the sealed store reports and never repairs. The
/// instance then holds nothing, so the directories are free at once, while
/// it is still alive, for whoever comes to look at the damage.
#[test]
fn an_engine_that_refuses_the_pair_leaves_the_directories_free() {
    let root = TempRoot::new("engine-refused");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "refused-user");
    drop({
        let first = instance("refused-user");
        first
            .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
            .expect("first init");
        first
    });
    let mut damaged = 0;
    for (path, mut bytes) in snapshot(&mls) {
        let is_record = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("e_"));
        if is_record {
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            std::fs::write(&path, bytes).expect("damage");
            damaged += 1;
        }
    }
    assert!(damaged > 0, "the first run stored its identity");

    let protocol = instance("refused-user");
    let err = protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect_err("a damaged identity");
    assert!(matches!(err, ProtocolError::MlsError(_)), "got {err:?}");
    assert!(!protocol.is_mls_initialized());

    let key = offline_protocol::StaticStoreKey::from_slice(&KEY).expect("key");
    offline_protocol::SealedFileMlsStorage::open(&mls, &namespace, &key)
        .expect("the MLS directory is free while the instance lives");
    offline_protocol::FileProtocolStateStorage::open(&state, &namespace)
        .expect("the state directory is free while the instance lives");
    protocol
        .close_file_stores()
        .expect("nothing is held, so nothing to close");
    // Had the refused stores stayed recorded, that close would have been a
    // real one and the instance would now be closed for good.
    protocol
        .start()
        .expect("the instance was refused, it did not give up");
}

/// The engine can refuse the pair it was offered. The stores are then
/// released by being closed, with a reference to them still alive, since a
/// release that waited for the last reference is the failure this exists to
/// prevent. An instance that holds no stores is left as it is.
#[test]
fn stores_the_engine_refused_are_closed_not_left_to_their_last_reference() {
    let root = TempRoot::new("refused");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let namespace = offline_protocol::account_storage_namespace("test-app", "refused-pair-user");
    let key = offline_protocol::StaticStoreKey::from_slice(&KEY).expect("key");
    let open = || offline_protocol::FileStorePair::open(&mls, &state, &namespace, &key);
    let holds = |protocol: &OfflineProtocol| {
        matches!(
            &*recover_mutex(&protocol.file_stores, "file_stores"),
            FileStoreState::Open(_)
        )
    };
    let protocol = instance("refused-pair-user");
    protocol.release_refused_stores();
    assert!(!holds(&protocol), "nothing held, nothing changed");

    let stores = open().expect("open");
    let still_referenced = stores.clone();
    *recover_mutex(&protocol.file_stores, "file_stores") = FileStoreState::Open(stores);
    assert!(matches!(
        open(),
        Err(offline_protocol::FileStoreError::InUse(_))
    ));
    protocol.release_refused_stores();
    assert!(!holds(&protocol));
    drop(open().expect("the directories are free while a reference lives"));
    drop(still_referenced);

    // Not closed for good: the instance was refused, it did not give up.
    protocol
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("a retry on the same instance");
}

/// End to end: a service whose record key is damaged still starts, as the
/// same identity. The record key is found by damaging one MLS record at a
/// time until the probe says so, since its file name is a keyed digest.
#[test]
fn a_damaged_record_key_does_not_stop_the_service() {
    let root = TempRoot::new("damaged-key");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let address = run_and_park_one("damaged-key-user", &mls, &state);
    let namespace = offline_protocol::account_storage_namespace("test-app", "damaged-key-user");
    let key = offline_protocol::StaticStoreKey::from_slice(&KEY).expect("key");
    let verdict = || {
        let secure = offline_protocol::SealedFileMlsStorage::open(&mls, &namespace, &key)
            .expect("open the MLS store");
        offline_protocol::FileProtocolStateStorage::open(&state, &namespace)
            .expect("open the state store")
            .sealed_state(&secure)
            .expect("verdict")
    };

    let mut found = false;
    for (path, bytes) in snapshot(&mls) {
        let is_record = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("e_"));
        if !is_record {
            continue;
        }
        let mut damaged = bytes.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        std::fs::write(&path, damaged).expect("damage");
        if verdict() == offline_protocol::SealedState::KeyDamaged {
            found = true;
            break;
        }
        std::fs::write(&path, bytes).expect("restore");
    }
    assert!(found, "one MLS record is the record key");

    let protocol = instance("damaged-key-user");
    protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("a damaged record key is the engine's to regenerate");
    assert_eq!(protocol.local_address(), Some(address.clone()));
    protocol.start().expect("start");
    protocol.stop().expect("stop");
    protocol.close_file_stores().expect("close");

    // And every launch after it: the pair was bound on its first run, and a
    // regenerated record key does not change what it is bound to.
    let next = instance("damaged-key-user");
    next.initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("the launch after the regeneration");
    assert_eq!(next.local_address(), Some(address));
}

/// The launch after a record key was regenerated must open, however much
/// the regenerating launch left sealed under the lost key. A document is
/// read when it is opened, never at launch, so all of it is left: read by its
/// records alone, the same identity's own state looked like another's, and
/// the service was refused on every launch from then on.
#[test]
fn a_document_under_a_lost_record_key_does_not_stop_the_next_launch() {
    let root = TempRoot::new("damaged-key-doc");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let profile = "damaged-key-doc-user";
    let service = || {
        OfflineProtocol::new(ProtocolConfig {
            profile: profile.to_string(),
            data_enabled: true,
            ..create_test_config()
        })
        .expect("protocol")
    };
    let namespace = offline_protocol::account_storage_namespace("test-app", profile);
    let key = offline_protocol::StaticStoreKey::from_slice(&KEY).expect("key");
    let verdict = || {
        let secure = offline_protocol::SealedFileMlsStorage::open(&mls, &namespace, &key)
            .expect("open the MLS store");
        offline_protocol::FileProtocolStateStorage::open(&state, &namespace)
            .expect("open the state store")
            .sealed_state(&secure)
            .expect("verdict")
    };

    let first = service();
    first
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("init");
    let address = first.local_address().expect("address");
    first
        .data_create_doc("demo-space".into(), "notes".into())
        .expect("create");
    first
        .data_map_set(
            "demo-space".into(),
            "notes".into(),
            "meta".into(),
            "title".into(),
            data_value_from_json(r#"{"kind":"text","value":"kept"}"#).expect("value"),
        )
        .expect("set");
    first
        .data_flush("demo-space".into(), "notes".into())
        .expect("flush");
    first.close_file_stores().expect("close");

    let mut found = false;
    for (path, bytes) in snapshot(&mls) {
        let is_record = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("e_"));
        if !is_record {
            continue;
        }
        let mut damaged = bytes.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        std::fs::write(&path, damaged).expect("damage");
        if verdict() == offline_protocol::SealedState::KeyDamaged {
            found = true;
            break;
        }
        std::fs::write(&path, bytes).expect("restore");
    }
    assert!(found, "one MLS record is the record key");

    let second = service();
    second
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("the damaged-key launch");
    second.close_file_stores().expect("close");
    assert_eq!(
        verdict(),
        offline_protocol::SealedState::Foreign,
        "the document is still sealed under the lost key, or this proves nothing"
    );

    let third = service();
    third
        .initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("the launch after it");
    assert_eq!(third.local_address(), Some(address));
}

/// A panic inside the engine poisons its lock, and `stop()` cannot take a
/// poisoned lock, so the protocol never reaches stopped. The directories
/// must still be releasable, or one panic holds them until the instance is
/// freed, which is the wait this call exists to end.
#[test]
fn a_poisoned_engine_lock_does_not_hold_the_directories() {
    let root = TempRoot::new("poisoned");
    let (mls, state) = (root.dir("mls"), root.dir("state"));
    let protocol = Arc::new(instance("poisoned-user"));
    protocol
        .initialize_mls_with_file_stores(mls.clone(), state.clone(), KEY.to_vec())
        .expect("init");
    protocol.start().expect("start");

    let panicking = protocol.clone();
    std::thread::spawn(move || {
        let _held = panicking.lock_inner().expect("lock");
        panic!("an engine call panicked (expected by this test)");
    })
    .join()
    .expect_err("the thread panicked");
    assert!(matches!(
        protocol.stop(),
        Err(ProtocolError::LockPoisoned(_))
    ));
    assert_eq!(protocol.get_state(), ProtocolState::Running);

    protocol.close_file_stores().expect("close");
    let next = instance("poisoned-user");
    next.initialize_mls_with_file_stores(mls, state, KEY.to_vec())
        .expect("the directories are free");
}
