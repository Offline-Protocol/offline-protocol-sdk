//! The two built-in stores opened as one pair, for one account.
//!
//! The invariant: an engine is never handed a protocol-state store that
//! another identity wrote. The engine deletes a sealed record that does not
//! open, as its restore reaches it, which is right for one damaged record and
//! wrong for a store sealed under someone else's record key: the first
//! restore deletes the queued and parked messages of an identity that is
//! merely somewhere else.
//!
//! Ownership is recorded, not inferred. The first open of a pair writes one
//! random pairing id into both stores, the MLS store first. A later open
//! compares the two ids, and nothing the engine does to its record key
//! changes them. Asking the sealed records instead
//! ([`FileProtocolStateStorage::sealed_state`]) cannot tell another
//! identity's state from this identity's state under a record key the engine
//! had to regenerate: records sealed under the lost key outlive the launch
//! that regenerated it (documents are read lazily, and each restore walk
//! stops at its delete allowance), and the next launch finds records that do
//! not open under a usable key. That reading refused the same identity over
//! its own roots. It remains only as the fallback for a state store that was
//! never bound.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rand_core::{OsRng, RngCore};

use super::key::StoreKeyProvider;
use super::{FileProtocolStateStorage, FileStoreError, SealedFileMlsStorage, SealedState};

/// Length of the pairing id both stores of a pair hold.
pub(super) const PAIRING_ID_BYTES: usize = 32;

/// What the MLS store holds where its pairing id belongs.
pub(super) enum StoredPairing {
    /// Never bound.
    Absent,
    /// Bound, to this id.
    Present([u8; PAIRING_ID_BYTES]),
    /// A pairing file that does not open under the store key. It names no
    /// state root, so the pair is judged by its records and bound afresh.
    Damaged,
}

/// The MLS store and the protocol-state store of one account, opened
/// together.
///
/// Use this rather than opening the two stores one by one: it refuses the
/// pairs the engine would lose data over, which the stores cannot see alone.
/// Hand [`Self::secure_storage`] and [`Self::state_storage`] to
/// `initialize_mls`, and keep the pair to [`close`](Self::close) it.
#[derive(Clone)]
pub struct FileStorePair {
    secure: Arc<SealedFileMlsStorage>,
    state: Arc<FileProtocolStateStorage>,
}

impl std::fmt::Debug for FileStorePair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStorePair")
            .field("secure", &self.secure)
            .field("state", &self.state)
            .finish()
    }
}

impl FileStorePair {
    /// Opens the MLS store at `mls_root` and the protocol-state store at
    /// `state_root`, for the account `namespace`, refusing a pair the engine
    /// would lose data over.
    ///
    /// Each refusal says what to change, and none changes or deletes a
    /// record:
    ///
    /// - [`FileStoreError::RootsOverlap`]: one directory for both, or one
    ///   inside the other. Asked as spelled before anything is created, and
    ///   again of the directories once both exist, because a volume that
    ///   folds case makes one directory of two spellings.
    /// - [`FileStoreError::StateWithoutIdentity`]: protocol state with no MLS
    ///   record beside it. A new identity would be minted, and it could not
    ///   unseal that state. Asked before anything is created.
    /// - [`FileStoreError::ForeignState`]: protocol state that belongs to
    ///   another MLS store. Asked once both are open.
    /// - Whatever either store's own open refuses: a wrong key, a directory
    ///   another store holds.
    ///
    /// A record key the MLS store reports as damaged is not refused: nothing
    /// opens the state sealed under it again, and the engine's new key and
    /// its settlement of that state as failed is how the application learns
    /// which messages were lost. Once the pair is bound (this binds it), the
    /// pairing ids decide every later open, so the records left under the
    /// lost key do not make it look like another identity's state. A pair
    /// whose record key was regenerated while its stores were opened one by
    /// one, never bound, is still judged by those records on its first open
    /// here, and can be refused.
    ///
    /// A pair that passes is bound: both stores hold the same pairing id
    /// from then on.
    pub fn open(
        mls_root: impl AsRef<Path>,
        state_root: impl AsRef<Path>,
        namespace: &str,
        provider: &dyn StoreKeyProvider,
    ) -> Result<Self, FileStoreError> {
        let (mls_root, state_root) = (mls_root.as_ref(), state_root.as_ref());
        let overlap = |on_this_volume| FileStoreError::RootsOverlap {
            mls_root: mls_root.to_path_buf(),
            state_root: state_root.to_path_buf(),
            on_this_volume,
        };
        if roots_overlap(mls_root, state_root) {
            return Err(overlap(false));
        }
        refuse_state_without_identity(mls_root, state_root, namespace)?;

        // MLS first: it proves the key, and a refused key should leave no
        // protocol-state directory behind for an operator to wonder about.
        let secure = SealedFileMlsStorage::open(mls_root, namespace, provider)?;
        let state = FileProtocolStateStorage::open(state_root, namespace)?;

        // Both roots exist now, so the question asked above by spelling can
        // be asked of the directories themselves. APFS and NTFS fold case by
        // default, and no comparison of names that do not exist yet can know.
        if roots_overlap(mls_root, state_root) {
            return Err(overlap(true));
        }

        let pairing = secure.pairing()?;
        // Asked under both locks, so nothing writes in between. A state store
        // that holds no record has nothing to lose to any identity.
        if FileProtocolStateStorage::holds_records(state_root, namespace)? {
            admit(
                ownership(&secure, &state, &pairing)?,
                state.directory(),
                mls_root,
            )?;
        }
        bind(&secure, &state, pairing)?;
        Ok(Self {
            secure: Arc::new(secure),
            state: Arc::new(state),
        })
    }

    /// The MLS store, for `initialize_mls`.
    pub fn secure_storage(&self) -> Arc<SealedFileMlsStorage> {
        self.secure.clone()
    }

    /// The protocol-state store, for `initialize_mls`.
    pub fn state_storage(&self) -> Arc<FileProtocolStateStorage> {
        self.state.clone()
    }

    /// Releases both directories, whoever still holds either store. The
    /// protocol-state store first: it is the one sealed under the other's
    /// record key. See [`SealedFileMlsStorage::close`].
    pub fn close(&self) {
        self.state.close();
        self.secure.close();
    }
}

/// Whether the state store belongs with the MLS store.
///
/// Decided by the pairing ids when both stores hold one: equal ids belong
/// together, and a state store bound to another id belongs with another MLS
/// store. When the MLS store holds no readable id (never bound, restored from
/// a backup older than its binding, the file removed or damaged), or the
/// state store holds none, the sealed records decide: another identity's
/// records still open under no key this store holds.
///
/// The binding says which MLS store the state root was paired with, not who
/// wrote to it last. A state root shared with another writer (the keyring
/// mode of a binding over the same directory, a Rust host opening the state
/// store alone) can hold records another identity sealed while the ids still
/// match. That sharing already loses data without this: each identity's
/// restore deletes what the other sealed.
fn ownership(
    secure: &SealedFileMlsStorage,
    state: &FileProtocolStateStorage,
    pairing: &StoredPairing,
) -> Result<SealedState, FileStoreError> {
    match (state.pairing()?, pairing) {
        (Some(bound), StoredPairing::Present(ours)) if bound == *ours => Ok(SealedState::Opens),
        (Some(_), StoredPairing::Present(_)) => Ok(SealedState::Foreign),
        (Some(_), StoredPairing::Absent | StoredPairing::Damaged) | (None, _) => {
            state.sealed_state(secure)
        }
    }
}

/// Refuses protocol state another identity wrote, and nothing else.
///
/// Exhaustive on purpose: a verdict added later has to be decided here, not
/// let through by a wildcard.
fn admit(
    verdict: SealedState,
    state_directory: &Path,
    mls_root: &Path,
) -> Result<(), FileStoreError> {
    match verdict {
        SealedState::Empty | SealedState::Opens => Ok(()),
        SealedState::KeyDamaged => {
            tracing::warn!(
                state = %state_directory.display(),
                "the protocol-state record key is damaged: the sealed state beside it \
                 cannot be opened again, and the engine will settle it as failed"
            );
            Ok(())
        }
        SealedState::Foreign => Err(FileStoreError::ForeignState {
            state: state_directory.to_path_buf(),
            mls_root: mls_root.to_path_buf(),
        }),
    }
}

/// Makes both stores hold one pairing id: the MLS store's when it has one,
/// otherwise a new one.
///
/// A new id goes to the state store first. A crash (or a failed write)
/// between the two writes then leaves the MLS store without a readable id,
/// which the next open judges by the records and binds again. The other
/// order could leave the MLS store on the new id and the state store on an
/// old one it was rebound from, which reads as another MLS store's state on
/// every open after.
fn bind(
    secure: &SealedFileMlsStorage,
    state: &FileProtocolStateStorage,
    pairing: StoredPairing,
) -> Result<(), FileStoreError> {
    match pairing {
        StoredPairing::Present(id) => {
            if state.pairing()? != Some(id) {
                state.set_pairing(&id)?;
            }
        }
        StoredPairing::Absent | StoredPairing::Damaged => {
            let mut id = [0u8; PAIRING_ID_BYTES];
            OsRng.fill_bytes(&mut id);
            state.set_pairing(&id)?;
            secure.set_pairing(&id)?;
        }
    }
    Ok(())
}

/// Refuses a protocol-state account directory that holds records when the
/// MLS account directory holds none.
///
/// That shape is a deployment moving onto the file stores (or one whose
/// `mls_root` was lost) while keeping its state root. The MLS store would
/// mint a new identity, so the device gets a new address, and the new
/// identity's record key cannot unseal the old records: the first restore
/// deletes the parked messages it cannot read.
///
/// The MLS side is asked for record files, not for its directory, a type
/// directory or its pairing file: an attempt refused at the state root has
/// already created the MLS directory, its lock and its key check, a first
/// write that failed leaves an empty type directory, and the pairing file is
/// written before the engine mints the identity. An existence test would let
/// the retry through. A directory that cannot be read on either side is
/// refused too, never taken for empty. Nothing is created before this check.
fn refuse_state_without_identity(
    mls_root: &Path,
    state_root: &Path,
    namespace: &str,
) -> Result<(), FileStoreError> {
    if SealedFileMlsStorage::holds_records(mls_root, namespace)? {
        return Ok(());
    }
    if FileProtocolStateStorage::holds_records(state_root, namespace)? {
        return Err(FileStoreError::StateWithoutIdentity {
            state: state_root.join(namespace),
            mls_root: mls_root.to_path_buf(),
        });
    }
    Ok(())
}

/// Whether one root is the other, or lies inside it.
///
/// Neither root need exist yet, and either may be relative or spelled with
/// `.` and `..`, so each is resolved first: see [`resolved_root`].
fn roots_overlap(a: &Path, b: &Path) -> bool {
    let (a, b) = (resolved_root(a), resolved_root(b));
    a.starts_with(&b) || b.starts_with(&a)
}

/// `path` made absolute, with its longest existing ancestor canonicalized
/// (so a symlink or a differently spelled existing prefix compares equal)
/// and the rest, which does not exist and so holds no symlink, resolved by
/// its components.
fn resolved_root(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    let base = loop {
        if let Ok(canonical) = existing.canonicalize() {
            break canonical;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(Component::Normal(name));
                existing = parent;
            }
            // `..` or `.` as the last component: keep it for the walk below.
            (Some(parent), None) => {
                rest.extend(existing.components().next_back());
                existing = parent;
            }
            (None, _) => break PathBuf::new(),
        }
    };
    let mut resolved = base;
    for component in rest.into_iter().rev() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_store::account_storage_namespace;
    use crate::file_store::mls::PAIRING_FILE;
    use crate::file_store::test_support::{files_under, TempRoot};
    use crate::file_store::StaticStoreKey;
    use crate::protocol_state_storage::ProtocolStateStorage;
    use offline_protocol_mls::storage::MlsStorage;

    fn namespace() -> String {
        account_storage_namespace("com.example.pair", "alice")
    }

    struct Roots {
        keys: TempRoot,
        state: TempRoot,
    }

    fn roots(label: &str) -> Roots {
        Roots {
            keys: TempRoot::new(&format!("{label}-keys")),
            state: TempRoot::new(&format!("{label}-state")),
        }
    }

    fn open(roots: &Roots) -> Result<FileStorePair, FileStoreError> {
        FileStorePair::open(
            roots.keys.path(),
            roots.state.path(),
            &namespace(),
            &StaticStoreKey::new([4; 32]),
        )
    }

    /// Only another identity's state is refused, and the match that says so
    /// is exhaustive: a damaged record key is let through, since nothing
    /// opens that state again and the engine's regeneration is what settles
    /// the messages lost with it.
    #[test]
    fn only_state_another_identity_wrote_is_refused() {
        let (state, keys) = (Path::new("/srv/state/account"), Path::new("/srv/keys"));
        let err = admit(SealedState::Foreign, state, keys).expect_err("foreign");
        let message = err.to_string();
        assert!(
            matches!(err, FileStoreError::ForeignState { .. })
                && message.contains("another identity")
                && message.contains("fresh directory")
                && message.contains("/srv/state/account")
                && message.contains("/srv/keys"),
            "{message}"
        );
        for verdict in [
            SealedState::Empty,
            SealedState::Opens,
            SealedState::KeyDamaged,
        ] {
            admit(verdict, state, keys).unwrap_or_else(|err| panic!("{verdict:?}: {err}"));
        }
    }

    /// The first open binds both stores to one id, the MLS store first, and
    /// neither pairing file is a record: the probes that decide whether a new
    /// identity may start still see an empty pair.
    #[test]
    fn the_first_open_binds_the_pair_and_writes_no_record() {
        let roots = roots("pair-bind");
        let pair = open(&roots).expect("open");
        let StoredPairing::Present(id) = pair.secure.pairing().expect("mls pairing") else {
            panic!("the MLS store is bound");
        };
        assert_eq!(pair.state.pairing().expect("state pairing"), Some(id));
        assert!(!SealedFileMlsStorage::holds_records(roots.keys.path(), &namespace()).unwrap());
        assert!(
            !FileProtocolStateStorage::holds_records(roots.state.path(), &namespace()).unwrap()
        );

        pair.close();
        let again = open(&roots).expect("reopen");
        assert!(
            matches!(again.secure.pairing().unwrap(), StoredPairing::Present(same) if same == id),
            "a bound pair keeps its id"
        );
    }

    /// A state store bound to one MLS store is refused beside another, and
    /// the refusal writes nothing. An MLS store that holds no pairing id at
    /// all did not write it either.
    #[test]
    fn state_bound_to_another_mls_store_is_refused_and_kept() {
        let ours = roots("pair-ours");
        let pair = open(&ours).expect("open");
        pair.secure
            .store("identity", "self", b"ours")
            .expect("an identity");
        pair.state
            .store("lamport_clock", "current", b"7")
            .expect("a record");
        pair.close();

        let theirs = roots("pair-theirs");
        let other = open(&theirs).expect("another identity's pair");
        other
            .secure
            .store("identity", "self", b"theirs")
            .expect("another identity");
        other.close();
        let before = files_under(ours.state.path());
        let err = FileStorePair::open(
            theirs.keys.path(),
            ours.state.path(),
            &namespace(),
            &StaticStoreKey::new([4; 32]),
        )
        .expect_err("another MLS store");
        assert!(matches!(err, FileStoreError::ForeignState { .. }), "{err}");
        assert_eq!(files_under(ours.state.path()), before);

        // An MLS store with an identity and no pairing id (never bound, or
        // restored from a backup older than its binding) is judged by the
        // records: sealed records it holds no key for are another identity's.
        let sealed = crate::protocol::state_crypto::StateRecordCipher::new(&[9; 32])
            .seal("outbox", "parked", b"another identity's")
            .expect("seal");
        let bound_state = FileProtocolStateStorage::open(ours.state.path(), &namespace())
            .expect("open the state store");
        bound_state
            .store("outbox", "parked", &sealed)
            .expect("a sealed record");
        bound_state.close();
        let before = files_under(ours.state.path());
        let unbound = TempRoot::new("pair-unbound-keys");
        let secure =
            SealedFileMlsStorage::open(unbound.path(), &namespace(), &StaticStoreKey::new([4; 32]))
                .expect("open");
        secure
            .store("identity", "self", b"key")
            .expect("an identity");
        secure.close();
        let err = FileStorePair::open(
            unbound.path(),
            ours.state.path(),
            &namespace(),
            &StaticStoreKey::new([4; 32]),
        )
        .expect_err("records that open under no key it holds");
        assert!(matches!(err, FileStoreError::ForeignState { .. }), "{err}");
        assert_eq!(files_under(ours.state.path()), before);

        open(&ours).expect("the pair that belongs together still opens");
    }

    /// A state store with no record has nothing to lose, so it is not
    /// refused whatever id it carries: the pair is bound afresh.
    #[test]
    fn a_state_store_with_no_record_is_rebound_not_refused() {
        let ours = roots("pair-empty-ours");
        drop(open(&ours).expect("open"));
        let theirs = roots("pair-empty-theirs");
        let pair = FileStorePair::open(
            theirs.keys.path(),
            ours.state.path(),
            &namespace(),
            &StaticStoreKey::new([4; 32]),
        )
        .expect("no record to lose");
        let StoredPairing::Present(id) = pair.secure.pairing().unwrap() else {
            panic!("bound");
        };
        assert_eq!(pair.state.pairing().unwrap(), Some(id));
    }

    /// The same identity with its MLS pairing file gone (a backup older than
    /// the binding, a copy that left it behind) is judged by its records, and
    /// its own records do not make it foreign.
    #[test]
    fn a_missing_mls_pairing_is_judged_by_the_records_not_refused() {
        let roots = roots("pair-missing");
        let pair = open(&roots).expect("open");
        pair.secure
            .store("identity", "self", b"ours")
            .expect("an identity");
        pair.state
            .store("lamport_clock", "current", b"7")
            .expect("a record");
        let path = pair.secure.directory().join(PAIRING_FILE);
        pair.close();
        std::fs::remove_file(&path).expect("remove the pairing file");

        let pair = open(&roots).expect("the same identity over its own state");
        let StoredPairing::Present(id) = pair.secure.pairing().unwrap() else {
            panic!("bound again");
        };
        assert_eq!(pair.state.pairing().unwrap(), Some(id));
    }

    /// A rebind that fails after its first write (a crash, a full disk) must
    /// leave a pair the next open still admits. The state store is written
    /// first, so when its write fails the MLS store keeps no readable id and
    /// the records decide again. The other order left the MLS store on the
    /// new id and the state store on the old one, refused for good.
    #[test]
    fn a_rebind_that_fails_part_way_still_opens_next_time() {
        let roots = roots("pair-cut-short");
        let pair = open(&roots).expect("open");
        pair.secure
            .store("identity", "self", b"ours")
            .expect("an identity");
        pair.state
            .store("lamport_clock", "current", b"7")
            .expect("a record");
        let path = pair.secure.directory().join(PAIRING_FILE);
        pair.close();
        std::fs::remove_file(&path).expect("unbind the MLS store");

        crate::file_store::state::tests::FAIL_PAIRING_WRITE.with(|fail| fail.set(true));
        let err = open(&roots).expect_err("the state store's pairing write fails");
        assert!(matches!(err, FileStoreError::Io { .. }), "{err}");

        open(&roots).expect("the next open admits the pair and binds it");
    }

    /// A pairing file that no longer opens names no state store. The pair
    /// is judged by its records, and bound again once it passes.
    #[test]
    fn a_damaged_mls_pairing_falls_back_to_the_records() {
        let roots = roots("pair-damaged");
        let pair = open(&roots).expect("open");
        let path = pair.secure.directory().join(PAIRING_FILE);
        pair.secure
            .store("identity", "self", b"ours")
            .expect("an identity");
        pair.state
            .store("lamport_clock", "current", b"7")
            .expect("an unsealed record");
        pair.close();
        let mut bytes = std::fs::read(&path).expect("pairing file");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, bytes).expect("damage");

        let pair = open(&roots).expect("no sealed record, so the records say Empty");
        let StoredPairing::Present(id) = pair.secure.pairing().unwrap() else {
            panic!("bound again");
        };
        assert_eq!(pair.state.pairing().unwrap(), Some(id));
    }
}
