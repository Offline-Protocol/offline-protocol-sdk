//! Main MLS manager - the primary interface for MLS operations.

use crate::error::{MlsError, Result};
use crate::group::{GroupManager, DEFAULT_CIPHERSUITE};
use crate::provider::MlsProvider;
use crate::session::SessionManager;
use crate::storage::MlsStorage;
use crate::storage_adapter::MlsStorageAdapter;
use crate::types::{
    EncryptedMessage, GroupId, GroupInfo, GroupMetadata, GroupRole, KeyPackageBundle,
    MlsMessageType, StorageKeyType, WelcomeMessage,
};

use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Default lifetime for key packages (30 days in seconds).
const DEFAULT_KEY_PACKAGE_LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;

/// How long an expired key package's private init key is kept before it is
/// destroyed (7 days in seconds).
///
/// Expiry stops a package being advertised; this window is what still lets a
/// Welcome built against it just before that moment be processed. Past it the
/// material is genuinely deleted — without this, every package this device ever
/// minted would keep its init key in provider storage for the life of the
/// install, because the only other thing that removes one is a peer actually
/// using it.
const KEY_PACKAGE_PURGE_GRACE_SECS: u64 = 7 * 24 * 60 * 60;

/// Ceiling on live, unconsumed key packages the push path will keep in flight.
///
/// The pool is normally self-limiting: one package per peer, minted only when
/// that peer's previous one was consumed or expired, so its size tracks the
/// number of peers with an outstanding unused advertisement. This is the
/// backstop for the case where it does not — a device meeting an unbounded
/// stream of peers that never establish sessions. Reaching it degrades the
/// push path to sharing one package again, which is what it did unconditionally
/// before, so the failure mode is the old behaviour rather than a new one; it
/// is reported (see `PushKeyPackagePoolExhausted`) rather than left silent.
pub const MAX_PUSH_KEY_PACKAGES: usize = 64;

/// What the caller knows about the identity behind a key package.
///
/// RFC 9420 delegates credential validation to an Authentication Service that
/// MLS itself does not define, and OpenMLS is explicit that it "does not
/// validate that a credential's identity legitimately owns the signature key".
/// In this SDK that service is trust-on-first-use, and this type is how its
/// verdict reaches the code that consumes a key package.
///
/// It is an enum rather than `Option<&[u8]>` on purpose. Every call site has to
/// state which case it is in, so a new one cannot quietly inherit "no pin" as a
/// default — the same reasoning that makes protocol-state sealing an exhaustive
/// match over categories instead of a `matches!` that answers "not sensitive"
/// for anything nobody remembered to list.
#[derive(Debug, Clone, Copy)]
pub enum KeyPackageTrust<'a> {
    /// A signature key is pinned for this peer; the key package's leaf key must
    /// equal it. Anything else is a substitution and is refused.
    Pinned(&'a [u8]),
    /// No pin exists yet, so there is nothing to compare against and the
    /// package is accepted on its own validity.
    ///
    /// Correct only when a pin genuinely does not exist. Passing this while
    /// holding one silently discards the only check that distinguishes the peer
    /// from anyone who can write the string `bob` into a basic credential.
    FirstUse,
}

/// A key package handed to the push path, plus how healthy the pool was.
///
/// The flag is not an error: the package is usable either way. It reports that
/// the pool hit [`MAX_PUSH_KEY_PACKAGES`] and this package is therefore shared
/// with another peer, which is the one condition under which the push path
/// still has the reuse shape it otherwise no longer has.
#[derive(Debug, Clone)]
pub struct PushKeyPackage {
    /// The package to advertise.
    pub bundle: KeyPackageBundle,
    /// Whether the pool ceiling forced this package to be shared.
    pub pool_exhausted: bool,
}

/// Whether `candidate` was minted more recently than `current`, treating an
/// absent `current` as older than anything.
fn newer_than(candidate: &KeyPackageBundle, current: Option<&KeyPackageBundle>) -> bool {
    current.is_none_or(|existing| candidate.created_at_ms > existing.created_at_ms)
}

/// Main MLS manager for end-to-end encryption.
pub struct MlsManager {
    /// The local user's ID.
    user_id: String,

    /// Storage backend for persisting MLS state.
    storage: Arc<dyn MlsStorage>,

    /// OpenMLS provider.
    provider: MlsProvider,

    /// Cached credential.
    credential: RwLock<Option<CredentialWithKey>>,

    /// Cached signature key pair (avoids re-reading storage on every crypto op).
    cached_signer: RwLock<Option<SignatureKeyPair>>,

    /// Session manager for 1:1 messaging.
    session_manager: SessionManager,

    /// Group manager for multi-party groups.
    group_manager: GroupManager,
}

impl MlsManager {
    /// Creates a new MLS manager.
    pub fn new(user_id: impl Into<String>, storage: Arc<dyn MlsStorage>) -> Result<Self> {
        let user_id = user_id.into();

        let adapter = MlsStorageAdapter::new(storage.clone());
        let provider = MlsProvider::new(adapter);

        let session_manager =
            SessionManager::new(user_id.clone(), storage.clone(), provider.clone());
        let group_manager = GroupManager::new(storage.clone(), provider.clone());

        let manager = Self {
            user_id,
            storage,
            provider,
            credential: RwLock::new(None),
            cached_signer: RwLock::new(None),
            session_manager,
            group_manager,
        };

        manager.ensure_identity()?;

        Ok(manager)
    }

    /// Returns the local user's ID.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Ensures the user has an identity.
    fn ensure_identity(&self) -> Result<()> {
        if self.load_identity()? {
            return Ok(());
        }
        self.create_identity()?;
        info!(user_id = %self.user_id, "Created new MLS identity");
        Ok(())
    }

    /// Loads the identity from storage.
    fn load_identity(&self) -> Result<bool> {
        let key_type = StorageKeyType::Identity.as_str();
        let keys_data = self.storage.load(key_type, "key_pair")?;

        match keys_data {
            Some(json) => {
                let signature_keys: SignatureKeyPair =
                    serde_json::from_slice(&json).map_err(|e| {
                        MlsError::Deserialization(format!(
                            "Failed to deserialize signature keys: {}",
                            e
                        ))
                    })?;

                let public_key = signature_keys.public();

                let credential =
                    Credential::new(CredentialType::Basic, self.user_id.as_bytes().to_vec());

                let credential_with_key = CredentialWithKey {
                    credential,
                    signature_key: public_key.into(),
                };

                {
                    let mut guard = self
                        .credential
                        .write()
                        .map_err(|_| MlsError::NotInitialized)?;
                    *guard = Some(credential_with_key);
                }
                {
                    let mut guard = self
                        .cached_signer
                        .write()
                        .map_err(|_| MlsError::NotInitialized)?;
                    *guard = Some(signature_keys);
                }

                debug!(user_id = %self.user_id, "Loaded existing MLS identity");
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Creates a new identity.
    fn create_identity(&self) -> Result<()> {
        let signature_keys = SignatureKeyPair::new(DEFAULT_CIPHERSUITE.signature_algorithm())
            .map_err(|e| MlsError::CryptoGeneration(format!("{:?}", e)))?;

        let keys_json = serde_json::to_vec(&signature_keys)
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        let key_type = StorageKeyType::Identity.as_str();
        self.storage.store(key_type, "key_pair", &keys_json)?;

        let public_key = signature_keys.public();

        let credential = Credential::new(CredentialType::Basic, self.user_id.as_bytes().to_vec());

        let credential_with_key = CredentialWithKey {
            credential,
            signature_key: public_key.into(),
        };

        {
            let mut guard = self
                .credential
                .write()
                .map_err(|_| MlsError::NotInitialized)?;
            *guard = Some(credential_with_key);
        }
        {
            let mut guard = self
                .cached_signer
                .write()
                .map_err(|_| MlsError::NotInitialized)?;
            *guard = Some(signature_keys);
        }

        Ok(())
    }

    /// Gets the credential with key.
    fn get_credential(&self) -> Result<CredentialWithKey> {
        let guard = self
            .credential
            .read()
            .map_err(|_| MlsError::NotInitialized)?;
        guard.clone().ok_or(MlsError::NotInitialized)
    }

    /// Gets a signer for MLS operations, using the in-memory cache.
    ///
    /// `SignatureKeyPair` doesn't implement `Clone`, so we serialize from
    /// the cached copy to avoid hitting storage on every crypto operation.
    fn get_signer(&self) -> Result<SignatureKeyPair> {
        let guard = self
            .cached_signer
            .read()
            .map_err(|_| MlsError::NotInitialized)?;

        let cached = guard.as_ref().ok_or(MlsError::NotInitialized)?;
        let bytes =
            serde_json::to_vec(cached).map_err(|e| MlsError::Serialization(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| MlsError::Deserialization(e.to_string()))
    }

    // ========================================================================
    // KEY PACKAGE MANAGEMENT
    // ========================================================================

    /// Generates a new key package for distribution over the push path.
    pub fn generate_key_package(&self) -> Result<KeyPackageBundle> {
        self.generate_key_package_inner(false)
    }

    /// Generates a key package reserved for a publication slot.
    ///
    /// Distinct from [`Self::generate_key_package`] only in that the result is
    /// withheld from [`Self::get_or_create_key_package`] — see
    /// [`KeyPackageBundle::reserved_for_publication`] for why sharing one
    /// package between a published record and a pushed exchange corrupts both.
    pub fn generate_publication_key_package(&self) -> Result<KeyPackageBundle> {
        self.generate_key_package_inner(true)
    }

    fn generate_key_package_inner(&self, reserved: bool) -> Result<KeyPackageBundle> {
        let credential = self.get_credential()?;
        let signature_keys = self.get_signer()?;

        let key_package_bundle = KeyPackage::builder()
            .build(
                DEFAULT_CIPHERSUITE,
                &self.provider,
                &signature_keys,
                credential,
            )
            .map_err(|e| MlsError::KeyPackageCreation(e.to_string()))?;

        let key_package_data = key_package_bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        // Stamped now because deriving it later from the serialized bytes
        // costs a parse plus a signature validation, which the pool scan
        // would pay per stored package per push. Best-effort: a record
        // without it takes the derive path once and is backfilled on load.
        let provider_hash_ref = key_package_bundle
            .key_package()
            .hash_ref(self.provider.crypto())
            .ok()
            .and_then(|hash_ref| hash_ref.tls_serialize_detached().ok());

        let package_id = Uuid::new_v4().to_string();

        let key_type = StorageKeyType::KeyPackage.as_str();
        let mut bundle = KeyPackageBundle::new(
            package_id,
            self.user_id.clone(),
            key_package_data,
            DEFAULT_KEY_PACKAGE_LIFETIME_SECS,
        );
        bundle.reserved_for_publication = reserved;
        bundle.provider_hash_ref = provider_hash_ref;
        let serialized =
            serde_json::to_vec(&bundle).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage
            .store(key_type, &bundle.package_id, &serialized)?;

        debug!(package_id = %bundle.package_id, "Generated new key package");
        Ok(bundle)
    }

    /// Gets an existing *unclaimed* key package or generates a new one.
    ///
    /// Two kinds of package are skipped, for the same underlying reason — an
    /// MLS init key is consumed by its first user, so two parties must never be
    /// pointed at one:
    ///
    /// - packages reserved for publication slots, or a pushed-to peer and a
    ///   stranger who fetched the published record would race for it;
    /// - packages already claimed by a peer over the push path
    ///   ([`Self::take_push_key_package`]), or this peer-less entry point would
    ///   hand out a key another peer is expected to use.
    ///
    /// This is the peer-less escape hatch (the FFI surface and tests). Callers
    /// that know the recipient should use [`Self::take_push_key_package`],
    /// which is what actually holds the one-key-per-peer property; a package
    /// handed out here is unclaimed, so the push path may later claim it for
    /// the first peer it is pushed to.
    pub fn get_or_create_key_package(&self) -> Result<KeyPackageBundle> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let packages = self.storage.list_keys(key_type)?;

        for package_id in packages {
            if let Some(bundle) = self.load_stored_key_package(&package_id)? {
                if bundle.reserved_for_publication || bundle.assigned_peer.is_some() {
                    continue;
                }
                return Ok(bundle);
            }
        }

        self.generate_key_package()
    }

    /// Hands `peer_id` the key package to advertise to it, minting one if it
    /// has no live package of its own.
    ///
    /// This is the push path's entry point, and the whole point of it is that
    /// two peers never receive the same init key. Before this existed the push
    /// path returned the first stored package to every caller until somebody's
    /// Welcome consumed it — the LastResort-style reuse RFC 9420 §16.8 permits
    /// only as a denial-of-service fallback, and which the Least Authority MDK
    /// audit flagged as enabling unsolicited joins and cross-group linkage. It
    /// also made a second peer's Welcome unprocessable once the first had used
    /// the key, with nothing to re-drive the exchange.
    ///
    /// Assignment is stored on the package rather than in a separate map, so it
    /// survives restarts for free and cannot disagree with the pool it
    /// describes. Resolution order:
    ///
    /// 1. this peer's own live package — a repeat push costs no new material;
    /// 2. an unclaimed package (minted by the peer-less entry point, or by a
    ///    build predating assignment), claimed here so upgrading does not
    ///    strand it;
    /// 3. a freshly minted package, claimed for this peer.
    ///
    /// Past [`MAX_PUSH_KEY_PACKAGES`] live packages it stops growing the pool
    /// and reuses the newest one, reporting that through
    /// [`PushKeyPackage::pool_exhausted`] so the caller can surface it. That is
    /// a deliberate degradation to the old behaviour rather than an error: the
    /// alternative — refusing to advertise, or evicting a package a peer may
    /// still be about to use — costs session establishment, which is worse than
    /// the weakened forward secrecy it would buy.
    ///
    /// Only step 3 is gated by the ceiling, because only minting grows the
    /// pool: claiming an unclaimed package relabels one that already exists, so
    /// degrading to a shared package while a claimable one sits idle would buy
    /// nothing. A corollary is that the shared package is always assigned to
    /// *another* peer — reaching that branch means neither an own nor an
    /// unclaimed package was found — and "newest" makes it the most recently
    /// assigned one, which has the most lifetime left but is also the peer most
    /// likely to be mid-establishment. If the over-ceiling peer's Welcome lands
    /// first, that peer's held advertisement becomes unprocessable until the
    /// next push to it mints a successor. Sharing the oldest instead would
    /// trade that for the shortest remaining lifetime; neither is free, and the
    /// exhaustion report is what makes the condition visible either way.
    ///
    /// Consumption is what rotates a peer's key: a Welcome built against the
    /// package removes its init key from provider storage, the loader then
    /// reports the package gone, and the next push to that peer mints a fresh
    /// one.
    pub fn take_push_key_package(&self, peer_id: &str) -> Result<PushKeyPackage> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        let mut live = 0usize;
        let mut assigned_here: Option<KeyPackageBundle> = None;
        let mut unclaimed: Option<KeyPackageBundle> = None;
        let mut newest: Option<KeyPackageBundle> = None;

        // One pass, which also does the pruning: `load_stored_key_package`
        // drops consumed and long-expired entries as it reads them, so the
        // count below is of genuinely usable packages.
        for package_id in package_ids {
            let Some(bundle) = self.load_stored_key_package(&package_id)? else {
                continue;
            };
            if bundle.reserved_for_publication {
                continue;
            }
            live += 1;

            if bundle.assigned_peer.as_deref() == Some(peer_id) {
                // Newest wins if a past interruption left this peer two.
                if newer_than(&bundle, assigned_here.as_ref()) {
                    assigned_here = Some(bundle.clone());
                }
            } else if bundle.assigned_peer.is_none() && newer_than(&bundle, unclaimed.as_ref()) {
                unclaimed = Some(bundle.clone());
            }

            if newer_than(&bundle, newest.as_ref()) {
                newest = Some(bundle);
            }
        }

        if let Some(bundle) = assigned_here {
            return Ok(PushKeyPackage {
                bundle,
                pool_exhausted: false,
            });
        }

        // Before the ceiling check, not after: claiming relabels a package that
        // already exists, so it cannot breach a bound on how many exist.
        if let Some(bundle) = unclaimed {
            return Ok(PushKeyPackage {
                bundle: self.claim_key_package(bundle, peer_id)?,
                pool_exhausted: false,
            });
        }

        // Minting is the only thing left that would grow the pool, so it is the
        // only thing the ceiling gates.
        if live >= MAX_PUSH_KEY_PACKAGES {
            // `live` is non-zero here, so `newest` is `Some`; minting on the
            // unreachable branch keeps this total without an unwrap.
            if let Some(bundle) = newest {
                debug!(
                    peer_id = %peer_id,
                    live = live,
                    "Push key-package pool at capacity; reusing an existing package"
                );
                return Ok(PushKeyPackage {
                    bundle,
                    pool_exhausted: true,
                });
            }
        }

        let bundle = self.claim_key_package(self.generate_key_package()?, peer_id)?;

        Ok(PushKeyPackage {
            bundle,
            pool_exhausted: false,
        })
    }

    /// Records `peer_id` as the owner of `bundle` and persists the claim.
    fn claim_key_package(
        &self,
        mut bundle: KeyPackageBundle,
        peer_id: &str,
    ) -> Result<KeyPackageBundle> {
        bundle.assigned_peer = Some(peer_id.to_string());
        let serialized =
            serde_json::to_vec(&bundle).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage.store(
            StorageKeyType::KeyPackage.as_str(),
            &bundle.package_id,
            &serialized,
        )?;
        debug!(
            package_id = %bundle.package_id,
            peer_id = %peer_id,
            "Assigned key package to peer"
        );
        Ok(bundle)
    }

    /// Loads a specific key package this device owns, or `None` if it is gone.
    ///
    /// `None` is the *consumption signal* the publication slots run on: the
    /// load prunes and reports missing any package whose private init key has
    /// left the OpenMLS provider, which is exactly what processing a Welcome
    /// built against it does. A slot whose package reads back `None` has been
    /// used by somebody and needs replacing.
    pub fn key_package_by_id(&self, package_id: &str) -> Result<Option<KeyPackageBundle>> {
        self.load_stored_key_package(package_id)
    }

    /// Deletes a key package this device owns.
    ///
    /// Exists to reclaim a *publication* package that was minted but whose
    /// record was never built or queued. Such a package is reserved, so
    /// [`Self::get_or_create_key_package`] will never hand it out, and no slot
    /// references it — nothing else would remove it before its lifetime runs
    /// out, so a repeatedly failing publish would otherwise strand fresh
    /// provider key material every refresh.
    ///
    /// "Strand" is meant literally, which is why this destroys the private init
    /// key too: deleting the bundle record alone leaves the material OpenMLS
    /// holds in place with nothing left pointing at it.
    pub fn delete_key_package(&self, package_id: &str) -> Result<()> {
        let Some(data) = self
            .storage
            .load(StorageKeyType::KeyPackage.as_str(), package_id)?
        else {
            return Ok(());
        };

        match serde_json::from_slice::<KeyPackageBundle>(&data) {
            Ok(bundle) => self.purge_key_package_material(package_id, &bundle),
            // Legacy raw storage: the record *is* the serialized key package,
            // the same reading `load_stored_key_package` applies when it
            // upgrades one. Wrapping it lets the purge derive the provider ref
            // and destroy the init key instead of stranding it — and if the
            // bytes are not a key package at all, the purge finds no ref and
            // falls back to deleting the record, which is all that is left to
            // do for a record that names nothing.
            Err(_) => {
                let legacy = KeyPackageBundle::new(
                    package_id.to_string(),
                    self.user_id.clone(),
                    data,
                    DEFAULT_KEY_PACKAGE_LIFETIME_SECS,
                );
                self.purge_key_package_material(package_id, &legacy)
            }
        }
    }

    /// Imports a contact's key package for later use.
    ///
    /// Security (SEC-M5): the caller-supplied `user_id` becomes both the
    /// storage key and — at session creation — the identity this key
    /// package is trusted for. Reject storage-hostile ids (the id is a raw
    /// `key_id`), cryptographically validate the key package, and require
    /// the embedded credential identity to equal `user_id` so a key
    /// package generated by one user can never be imported under
    /// another's name.
    ///
    /// `trust` carries the caller's Authentication Service verdict — see
    /// [`KeyPackageTrust`], which is an enum rather than an `Option` so that
    /// "I hold no pin for this peer" has to be written down rather than
    /// defaulted into.
    pub fn import_key_package(
        &self,
        user_id: &str,
        key_package_data: &[u8],
        trust: KeyPackageTrust<'_>,
    ) -> Result<()> {
        offline_protocol_core::validate_id_chars(user_id, "User ID")
            .map_err(|e| MlsError::InvalidUserId(e.to_string()))?;

        let key_package_in = KeyPackageIn::tls_deserialize_exact(key_package_data)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;
        let key_package = key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        Self::verify_credential_identity(&key_package, user_id)?;
        Self::verify_pinned_signature_key(&key_package, user_id, trust)?;

        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        self.storage.store(key_type, user_id, key_package_data)?;

        debug!(user_id = %user_id, "Imported key package for contact");
        Ok(())
    }

    /// Gets a contact's key package.
    fn get_contact_key_package(&self, user_id: &str) -> Result<KeyPackage> {
        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        let data = self
            .storage
            .load(key_type, user_id)?
            .ok_or_else(|| MlsError::NoKeyPackage(user_id.to_string()))?;

        let key_package_in = KeyPackageIn::tls_deserialize_exact(&data)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        // Validate using the crypto backend
        let key_package = key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        // Defense in depth: entries stored before identity binding was
        // enforced at import time (or written to storage out-of-band) must
        // not come back attributed to the wrong user.
        Self::verify_credential_identity(&key_package, user_id)?;

        Ok(key_package)
    }

    /// Requires the key package's leaf signature key to be the one already
    /// pinned for `user_id`, when the caller holds a pin.
    ///
    /// This is the check that makes a key package's identity claim mean
    /// something. [`Self::verify_credential_identity`] compares the leaf's
    /// *basic credential*, which per RFC 9420 is "a bare assertion of an
    /// identity" — self-asserted, so an attacker who generates their own
    /// signature keypair and writes `bob` into the credential passes it. The
    /// leaf signature key is not self-asserted relative to a pin: it is the
    /// peer's MLS identity key, the same value the control-message gate
    /// verified a signature against and pinned.
    ///
    /// This matters because a validated key package is not necessarily a
    /// *freshly received* one. Packages are cached in the install-scoped
    /// protocol-state store and re-read later, and the receive-time signature
    /// gate does not travel with them. Without this comparison, anyone able to
    /// write that store could swap in their own package and have the local node
    /// build the session — and encrypt to — their leaf instead of the peer's.
    fn verify_pinned_signature_key(
        key_package: &KeyPackage,
        user_id: &str,
        trust: KeyPackageTrust<'_>,
    ) -> Result<()> {
        let KeyPackageTrust::Pinned(expected) = trust else {
            return Ok(());
        };
        // An empty pin is not a pin. Treating it as one would compare against
        // nothing and pass everything, which is worse than declining to check.
        if expected.is_empty() {
            return Err(MlsError::KeyPackagePinMismatch {
                peer_id: user_id.to_string(),
            });
        }
        if key_package.leaf_node().signature_key().as_slice() != expected {
            return Err(MlsError::KeyPackagePinMismatch {
                peer_id: user_id.to_string(),
            });
        }
        Ok(())
    }

    /// Requires the key package's leaf credential identity to equal
    /// `user_id`. Credentials in this SDK are basic credentials carrying
    /// the owner's user id as raw bytes (see `IdentityManager`).
    fn verify_credential_identity(key_package: &KeyPackage, user_id: &str) -> Result<()> {
        let identity = key_package.leaf_node().credential().serialized_content();
        if identity != user_id.as_bytes() {
            return Err(MlsError::CredentialIdentityMismatch {
                expected: user_id.to_string(),
                found: String::from_utf8_lossy(identity).into_owned(),
            });
        }
        Ok(())
    }

    /// Gets pending key packages.
    pub fn get_pending_key_packages(&self) -> Result<Vec<KeyPackageBundle>> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        let mut bundles = Vec::new();
        for package_id in package_ids {
            if let Some(bundle) = self.load_stored_key_package(&package_id)? {
                bundles.push(bundle);
            }
        }

        Ok(bundles)
    }

    /// Marks a key package as synced.
    pub fn mark_key_package_synced(&self, package_id: &str) -> Result<()> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        self.storage.delete(key_type, package_id)?;
        debug!(package_id = %package_id, "Marked key package as synced");
        Ok(())
    }

    // ========================================================================
    // 1:1 ENCRYPTED MESSAGING
    // ========================================================================

    /// Checks if a session exists with another user.
    pub fn has_session(&self, other_user_id: &str) -> Result<bool> {
        self.session_manager.has_session(other_user_id)
    }

    /// Creates a new 1:1 session with another user.
    pub fn create_session(&self, other_user_id: &str) -> Result<WelcomeMessage> {
        let their_key_package = self.get_contact_key_package(other_user_id)?;
        let credential = self.get_credential()?;
        let signature_keys = self.get_signer()?;

        let welcome = self.session_manager.create_session(
            other_user_id,
            their_key_package,
            &credential,
            &signature_keys,
        )?;

        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        self.storage.delete(key_type, other_user_id)?;

        Ok(welcome)
    }

    /// Joins a session using a Welcome message.
    pub fn join_session(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        self.session_manager.join_session(welcome)
    }

    /// Replaces an existing session with an incoming Welcome message.
    ///
    /// This implements the "welcome-wins" strategy for race condition resolution.
    /// When both peers simultaneously create a session, this method allows one peer
    /// to replace their own session with the other peer's Welcome, ensuring both
    /// end up with the same cryptographic state.
    pub fn replace_session_with_welcome(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        let other_user_id = &welcome.inviter_id;

        // Security: `inviter_id` arrives on the wire and is used below as a
        // raw storage key for deletes — reject storage-hostile values just
        // like `import_key_package` does for its user id.
        offline_protocol_core::validate_id_chars(other_user_id, "User ID")
            .map_err(|e| MlsError::InvalidUserId(e.to_string()))?;

        // SEC-M6: reject a mismatched session slot BEFORE the best-effort
        // deletes below, so a forged Welcome that squats a third party's slot
        // performs no mutation. `join_session` re-checks this as its own
        // boundary; hoisting it here keeps the reject side effect-free.
        self.session_manager.verify_welcome_slot(welcome)?;

        // Clear any pending welcome we were about to send
        let _ = self.clear_pending_welcome(other_user_id);

        // Delete conflicting contact key package (we no longer need it)
        let key_type = StorageKeyType::ContactKeyPackage.as_str();
        let _ = self.storage.delete(key_type, other_user_id);

        // Join using their Welcome. `join_session` adopts non-destructively
        // (stage-then-swap), so a retransmitted Welcome that re-stages is a safe
        // no-op rather than deleting and re-creating our existing session.
        self.session_manager.join_session(welcome)
    }

    /// Encrypts a message for a 1:1 session.
    pub fn encrypt_for_user(
        &self,
        other_user_id: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        if !self.has_session(other_user_id)? {
            let welcome = self.create_session(other_user_id)?;
            warn!(
                other_user_id = %other_user_id,
                "Created new session - Welcome message needs to be sent"
            );
            let key_type = StorageKeyType::PendingWelcome.as_str();
            let welcome_data =
                serde_json::to_vec(&welcome).map_err(|e| MlsError::Serialization(e.to_string()))?;
            self.storage.store(key_type, other_user_id, &welcome_data)?;
        }

        let signature_keys = self.get_signer()?;
        self.session_manager
            .encrypt_message(other_user_id, plaintext, &signature_keys)
    }

    /// Encrypts a message for a 1:1 session that is known to exist.
    ///
    /// Unlike `encrypt_for_user`, this skips the `has_session()` storage check
    /// and goes directly to `encrypt_message()`. The caller must guarantee the
    /// session exists (e.g., via an in-memory cache). If the session was deleted
    /// externally, `SessionNotFound` is returned.
    pub fn encrypt_for_existing_session(
        &self,
        other_user_id: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        let signature_keys = self.get_signer()?;
        self.session_manager
            .encrypt_message(other_user_id, plaintext, &signature_keys)
    }

    /// Gets a pending Welcome message.
    pub fn get_pending_welcome(&self, other_user_id: &str) -> Result<Option<WelcomeMessage>> {
        let key_type = StorageKeyType::PendingWelcome.as_str();
        match self.storage.load(key_type, other_user_id)? {
            Some(data) => {
                let welcome: WelcomeMessage = serde_json::from_slice(&data)
                    .map_err(|e| MlsError::Deserialization(e.to_string()))?;
                Ok(Some(welcome))
            }
            None => Ok(None),
        }
    }

    /// Clears a pending Welcome message.
    pub fn clear_pending_welcome(&self, other_user_id: &str) -> Result<()> {
        let key_type = StorageKeyType::PendingWelcome.as_str();
        self.storage.delete(key_type, other_user_id)?;
        Ok(())
    }

    /// Decrypts a message from a 1:1 session.
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; decryption fails with
    /// [`MlsError::SenderIdentityMismatch`] if it does not match the
    /// MLS-authenticated credential (SEC-M1).
    pub fn decrypt_from_user(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        self.session_manager
            .decrypt_message(encrypted, claimed_sender)
    }

    /// Lists all active 1:1 sessions.
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        self.session_manager.list_sessions()
    }

    /// Deletes a 1:1 session.
    pub fn delete_session(&self, other_user_id: &str) -> Result<()> {
        self.session_manager.delete_session(other_user_id)
    }

    // ========================================================================
    // GROUP MESSAGING
    // ========================================================================

    /// Creates a new group.
    pub fn create_group(&self, group_name: &str) -> Result<GroupInfo> {
        let group_id = GroupId::new(format!("group:{}", Uuid::new_v4()))?;
        let credential = self.get_credential()?;
        let signature_keys = self.get_signer()?;

        let group = self
            .group_manager
            .create_group(&group_id, &credential, &signature_keys)?;

        // Store group metadata with creator as admin
        let metadata = GroupMetadata::new_with_creator(Some(group_name.to_string()), &self.user_id);
        self.save_group_metadata(&group_id, &metadata)?;

        let mut info = self.group_manager.get_group_info(&group, &group_id);
        info.name = metadata.name;
        info.created_at_ms = metadata.created_at_ms;
        info.last_activity_ms = metadata.last_activity_ms;

        info!(group_id = %group_id, name = %group_name, "Created new group");
        Ok(info)
    }

    /// Adds a member to a group.
    ///
    /// Returns a tuple of (WelcomeMessage, EncryptedMessage) where the
    /// WelcomeMessage should be sent to the invitee and the EncryptedMessage
    /// (Commit) should be distributed to all existing group members so they
    /// can advance their MLS epoch.
    ///
    /// `invitee_user_id` and `trust` bind the package to the identity it is
    /// being added under. This path previously ran neither check — not even the
    /// credential-identity one `import_key_package` has always had — so a
    /// package could be admitted to a group under a roster label that had
    /// nothing to do with it. The 1:1 path and the group path now apply the same
    /// two checks.
    pub fn add_group_member(
        &self,
        group_id: &GroupId,
        invitee_user_id: &str,
        member_key_package: &[u8],
        trust: KeyPackageTrust<'_>,
    ) -> Result<(WelcomeMessage, EncryptedMessage)> {
        let key_package = KeyPackageIn::tls_deserialize_exact(member_key_package)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsError::InvalidKeyPackage(e.to_string()))?;

        Self::verify_credential_identity(&key_package, invitee_user_id)?;
        Self::verify_pinned_signature_key(&key_package, invitee_user_id, trust)?;

        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let signature_keys = self.get_signer()?;
        let (commit, welcome) =
            self.group_manager
                .add_member(&mut group, key_package, &signature_keys)?;

        self.group_manager.save_group(group_id, &group)?;

        let welcome_bytes = welcome
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        let commit_bytes = commit
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        // Include group name in welcome for the invitee
        let group_name = self.load_group_metadata(group_id)?.and_then(|m| m.name);

        let now_ms = chrono::Utc::now().timestamp_millis() as u64;

        let welcome_msg = WelcomeMessage {
            group_id: group_id.clone(),
            welcome_data: welcome_bytes,
            inviter_id: self.user_id.clone(),
            group_name,
            timestamp_ms: now_ms,
        };

        let commit_msg = EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Commit,
            epoch: group.epoch().as_u64(),
            ciphertext: commit_bytes,
            sender_id: self.user_id.clone(),
            timestamp_ms: now_ms,
        };

        Ok((welcome_msg, commit_msg))
    }

    /// Removes a member from a group.
    pub fn remove_group_member(
        &self,
        group_id: &GroupId,
        member_id: &str,
    ) -> Result<EncryptedMessage> {
        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let member_index = group
            .members()
            .find_map(|m| {
                let cred_data = m.credential.serialized_content();
                if cred_data == member_id.as_bytes() {
                    Some(m.index)
                } else {
                    None
                }
            })
            .ok_or_else(|| MlsError::UserNotInGroup(member_id.to_string()))?;

        let signature_keys = self.get_signer()?;
        let commit = self
            .group_manager
            .remove_member(&mut group, member_index, &signature_keys)?;

        self.group_manager.save_group(group_id, &group)?;

        let ciphertext = commit
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        Ok(EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Commit,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Leaves a group.
    pub fn leave_group(&self, group_id: &GroupId) -> Result<()> {
        self.group_manager.delete_group(group_id)?;
        info!(group_id = %group_id, "Left group");
        Ok(())
    }

    /// Encrypts a message for a group.
    pub fn encrypt_for_group(
        &self,
        group_id: &GroupId,
        plaintext: &[u8],
    ) -> Result<EncryptedMessage> {
        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let signature_keys = self.get_signer()?;
        let mls_message =
            self.group_manager
                .encrypt_message(&mut group, plaintext, &signature_keys)?;

        self.group_manager.save_group(group_id, &group)?;

        let ciphertext = mls_message
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        Ok(EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Application,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Decrypts a message from a group.
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; decryption fails with
    /// [`MlsError::SenderIdentityMismatch`] if it does not match the
    /// MLS-authenticated credential (SEC-M1). The check runs before any
    /// commit is merged, so a spoofed commit cannot advance group state.
    pub fn decrypt_from_group(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        let mut group = self
            .group_manager
            .load_group(&encrypted.group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(encrypted.group_id.to_string()))?;

        let mls_message = MlsMessageIn::tls_deserialize_exact(&encrypted.ciphertext)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let result = self.group_manager.decrypt_message(
            &mut group,
            &encrypted.group_id,
            mls_message,
            claimed_sender,
        )?;

        self.group_manager.save_group(&encrypted.group_id, &group)?;

        Ok(result)
    }

    /// Joins a group using a Welcome message.
    pub fn join_group(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        // SEC-M6 (group-Welcome side): the `session:` namespace is owned
        // exclusively by identity-bound 1:1 sessions installed via
        // `join_session`. A group Welcome carries an attacker-controllable
        // `group_id` with no (self, inviter) binding and writes the same
        // storage/OpenMLS keyspace, so one naming a `session:*` slot would seed
        // or overwrite a third party's 1:1 session and hijack the victim's
        // outbound encryption — the exact hijack SEC-M6 blocks on the
        // session-Welcome path. Reject before staging. Enforced here (rather
        // than only in the mesh handler) so *every* caller of `join_group` is
        // covered. Legitimate mesh groups are always `group:<uuid>`
        // (see `create_group`), so this rejects only forged Welcomes.
        if welcome.group_id.as_str().starts_with("session:") {
            return Err(MlsError::ReservedSessionNamespace {
                group_id: welcome.group_id.to_string(),
            });
        }

        let mls_msg = MlsMessageIn::tls_deserialize_exact(&welcome.welcome_data)
            .map_err(|e| MlsError::Deserialization(e.to_string()))?;

        let welcome_msg = match mls_msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => {
                return Err(MlsError::WelcomeProcessing(
                    "Not a Welcome message".to_string(),
                ))
            }
        };

        let group = self
            .group_manager
            .join_group(welcome_msg, &welcome.group_id)?;
        let mut info = self.group_manager.get_group_info(&group, &welcome.group_id);
        info.name = welcome.group_name.clone();

        info!(group_id = %welcome.group_id, "Joined group");
        Ok(info)
    }

    /// Lists all groups.
    pub fn list_groups(&self) -> Result<Vec<GroupId>> {
        let all_groups = self.group_manager.list_groups()?;
        Ok(all_groups
            .into_iter()
            .filter(|g| !g.as_str().starts_with("session:"))
            .collect())
    }

    /// Gets information about a group.
    pub fn get_group_info(&self, group_id: &GroupId) -> Result<Option<GroupInfo>> {
        let group = match self.group_manager.load_group(group_id)? {
            Some(g) => g,
            None => return Ok(None),
        };

        let mut info = self.group_manager.get_group_info(&group, group_id);

        // Merge stored metadata
        if let Some(metadata) = self.load_group_metadata(group_id)? {
            info.name = metadata.name;
            info.created_at_ms = metadata.created_at_ms;
            info.last_activity_ms = metadata.last_activity_ms;
        }

        Ok(Some(info))
    }

    /// Returns `true` if local MLS group state exists for `group_id`.
    ///
    /// This is the authoritative test for "do we actually participate in this
    /// group via MLS" — it gates on the stored group marker, not the member
    /// send-cache (which relay reconciliation can populate without any MLS
    /// state). Callers use it to distinguish a genuine legacy relay-only
    /// (unencrypted) group, which has no MLS state, from an unauthenticated
    /// plaintext frame spoofed against a group the node secures with MLS.
    pub fn has_group(&self, group_id: &GroupId) -> Result<bool> {
        Ok(self.group_manager.load_group(group_id)?.is_some())
    }

    /// Updates the group name.
    pub fn set_group_name(&self, group_id: &GroupId, name: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.name = Some(name.to_string());
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Gets group metadata.
    pub fn get_group_metadata(&self, group_id: &GroupId) -> Result<Option<GroupMetadata>> {
        self.load_group_metadata(group_id)
    }

    /// Sets custom metadata for a group.
    pub fn set_group_custom_metadata(
        &self,
        group_id: &GroupId,
        key: &str,
        value: &str,
    ) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.custom.insert(key.to_string(), value.to_string());
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Removes a custom metadata key for a group.
    pub fn remove_group_custom_metadata(&self, group_id: &GroupId, key: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.custom.remove(key);
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Sets a member's role in a group.
    pub fn set_member_role(
        &self,
        group_id: &GroupId,
        user_id: &str,
        role: GroupRole,
    ) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.set_role(user_id, role);
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Enables or disables receive-side authorization of group membership
    /// commits. See [`crate::group::GroupManager::set_enforce_admin_commits`]
    /// for the fork consequence — this is off by default and enabling it is a
    /// deployment decision, not a hardening toggle.
    ///
    /// Applies to group commits only. The session manager owns a separate
    /// group manager and is left untouched, so 1:1 sessions are unaffected
    /// regardless of this setting.
    pub fn set_enforce_admin_commits(&mut self, enforce: bool) {
        self.group_manager.set_enforce_admin_commits(enforce);
    }

    /// Records the group's creator, **only if none is on record yet**.
    ///
    /// `created_by` is the fallback [`GroupMetadata`] consults when no admin
    /// role is stored (see the protocol layer's `check_is_admin`), so it must
    /// behave monotonically: a device that created the group, or already
    /// adopted a creator from the Welcome that admitted it, keeps what it has.
    /// Only the "no information" state is fillable. That makes the write
    /// idempotent under duplicate Welcomes and stops a later invite — from an
    /// inviter whose own metadata disagrees — from rewriting an established
    /// admin fallback.
    ///
    /// Returns `Ok(())` whether or not the value was adopted; use
    /// [`Self::get_group_metadata`] to observe the result.
    pub fn set_group_creator(&self, group_id: &GroupId, creator_id: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        if metadata.created_by.is_some() {
            return Ok(());
        }
        metadata.created_by = Some(creator_id.to_string());
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    /// Removes a member's role metadata from a group.
    pub fn remove_member_role(&self, group_id: &GroupId, user_id: &str) -> Result<()> {
        let mut metadata = self
            .load_group_metadata(group_id)?
            .unwrap_or_else(|| GroupMetadata::new(None));
        metadata.remove_role(user_id);
        metadata.touch();
        self.save_group_metadata(group_id, &metadata)
    }

    // ========================================================================
    // GENERIC MESSAGE HANDLING
    // ========================================================================

    /// Decrypts any incoming encrypted message.
    ///
    /// `claimed_sender` is the transport-level sender this message will be
    /// attributed to; it must match the MLS-authenticated credential
    /// (SEC-M1).
    pub fn decrypt(
        &self,
        encrypted: &EncryptedMessage,
        claimed_sender: &str,
    ) -> Result<Option<Vec<u8>>> {
        if encrypted.group_id.as_str().starts_with("session:") {
            self.decrypt_from_user(encrypted, claimed_sender)
        } else {
            self.decrypt_from_group(encrypted, claimed_sender)
        }
    }

    /// Processes a Welcome message.
    pub fn process_welcome(&self, welcome: &WelcomeMessage) -> Result<GroupInfo> {
        if welcome.group_id.as_str().starts_with("session:") {
            self.join_session(welcome)
        } else {
            self.join_group(welcome)
        }
    }
}

impl MlsManager {
    /// Loads a stored key package bundle, handling legacy raw storage and expiration.
    ///
    /// Also validates that the key package's private key still exists in the
    /// OpenMLS provider storage. Key packages whose private keys have been
    /// consumed (e.g. by a previous Welcome processing) are pruned.
    fn load_stored_key_package(&self, package_id: &str) -> Result<Option<KeyPackageBundle>> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let data = match self.storage.load(key_type, package_id)? {
            Some(data) => data,
            None => return Ok(None),
        };

        let mut bundle = match serde_json::from_slice::<KeyPackageBundle>(&data) {
            Ok(bundle) => bundle,
            Err(_) => {
                let legacy_bundle = KeyPackageBundle::new(
                    package_id.to_string(),
                    self.user_id.clone(),
                    data,
                    DEFAULT_KEY_PACKAGE_LIFETIME_SECS,
                );
                let serialized = serde_json::to_vec(&legacy_bundle)
                    .map_err(|e| MlsError::Serialization(e.to_string()))?;
                self.storage.store(key_type, package_id, &serialized)?;
                legacy_bundle
            }
        };

        if bundle.is_expired() {
            // Expiry and key destruction are separate moments on purpose. A
            // peer handed this package shortly before it expired may still
            // Welcome us, and that Welcome is only processable while the init
            // key is resident — so the record is kept (invisible to every
            // caller, since they all read through here) until the grace window
            // closes, and only then is the material destroyed.
            if bundle.expired_past_grace(KEY_PACKAGE_PURGE_GRACE_SECS) {
                self.purge_key_package_material(package_id, &bundle)?;
            }
            return Ok(None);
        }

        // Usability means the private init key is still in provider storage.
        // The lookup key is cached on the record; deriving it instead is the
        // expensive path (a parse plus a signature validation), paid once for
        // records written before the cache existed and stamped back below so
        // their next load is cheap.
        let needs_backfill = bundle.provider_hash_ref.is_none();
        let Some(hash_ref) = self.bundle_hash_ref(&bundle) else {
            warn!(
                package_id = %package_id,
                "Key package bytes no longer parse as a key package, pruning stale entry"
            );
            self.storage.delete(key_type, package_id)?;
            return Ok(None);
        };
        if !self.provider_has_init_key(&hash_ref) {
            warn!(
                package_id = %package_id,
                "Key package private key no longer in provider storage, pruning stale entry"
            );
            self.storage.delete(key_type, package_id)?;
            return Ok(None);
        }

        if needs_backfill {
            if let Ok(bytes) = hash_ref.tls_serialize_detached() {
                bundle.provider_hash_ref = Some(bytes);
                let serialized = serde_json::to_vec(&bundle)
                    .map_err(|e| MlsError::Serialization(e.to_string()))?;
                self.storage.store(key_type, package_id, &serialized)?;
            }
        }

        Ok(Some(bundle))
    }

    /// Destroys a key package: its bundle record *and* the private init key
    /// OpenMLS holds for it.
    ///
    /// Deleting the record alone — which is all this crate used to do — leaves
    /// the init key resident forever, because the only other thing that removes
    /// one is OpenMLS consuming it to process a Welcome. That turns every
    /// expired or reclaimed package into permanently retained key material,
    /// which is exactly the property the lifetime was supposed to bound.
    ///
    /// The provider key goes first: if that fails, the record is deliberately
    /// left in place so the next scan retries rather than orphaning key
    /// material no record points at any more. The package is already out of
    /// every caller's reach by the time this runs, so a retained record costs
    /// one storage read per scan and nothing else.
    fn purge_key_package_material(
        &self,
        package_id: &str,
        bundle: &KeyPackageBundle,
    ) -> Result<()> {
        if let Some(hash_ref) = self.bundle_hash_ref(bundle) {
            use openmls_traits::storage::StorageProvider;
            if let Err(e) = self.provider.storage().delete_key_package(&hash_ref) {
                warn!(
                    package_id = %package_id,
                    error = %e,
                    "Failed to delete a key package's private init key; keeping the record to retry"
                );
                return Ok(());
            }
        }

        self.storage
            .delete(StorageKeyType::KeyPackage.as_str(), package_id)?;
        debug!(package_id = %package_id, "Purged key package and its init key");
        Ok(())
    }

    /// Computes the OpenMLS hash reference a key package's private material is
    /// stored under, or `None` if the bytes no longer parse as a key package.
    ///
    /// This is the expensive path — a TLS parse plus a signature validation —
    /// that [`KeyPackageBundle::provider_hash_ref`] caches the result of; go
    /// through [`Self::bundle_hash_ref`] when a bundle is in hand.
    fn key_package_hash_ref(&self, key_package_data: &[u8]) -> Option<KeyPackageRef> {
        let kp_in = KeyPackageIn::tls_deserialize_exact(key_package_data).ok()?;
        let kp: KeyPackage = kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .ok()?;
        kp.hash_ref(self.provider.crypto()).ok()
    }

    /// Resolves the provider hash reference for one of this device's bundles:
    /// the copy cached at mint time when present and decodable, else derived
    /// from the serialized package bytes.
    fn bundle_hash_ref(&self, bundle: &KeyPackageBundle) -> Option<KeyPackageRef> {
        bundle
            .provider_hash_ref
            .as_deref()
            .and_then(|bytes| KeyPackageRef::tls_deserialize_exact(bytes).ok())
            .or_else(|| self.key_package_hash_ref(&bundle.key_package_data))
    }

    /// Whether the private init key `hash_ref` names is still in the OpenMLS
    /// provider storage. Absent means consumed (a Welcome was processed
    /// against it) or never stored.
    fn provider_has_init_key(&self, hash_ref: &KeyPackageRef) -> bool {
        use openmls_traits::storage::StorageProvider;
        let found: std::result::Result<Option<openmls::key_packages::KeyPackageBundle>, _> =
            self.provider.storage().key_package(hash_ref);
        matches!(found, Ok(Some(_)))
    }

    /// Ground-truth usability check from the serialized package bytes alone,
    /// bypassing the cached provider ref. Test-only so tests observe the
    /// provider directly rather than a cache they may be manipulating;
    /// production reads go through [`Self::bundle_hash_ref`].
    #[cfg(test)]
    fn is_key_package_usable(&self, key_package_data: &[u8]) -> bool {
        self.key_package_hash_ref(key_package_data)
            .map(|hash_ref| self.provider_has_init_key(&hash_ref))
            .unwrap_or(false)
    }

    /// Loads group metadata from storage.
    fn load_group_metadata(&self, group_id: &GroupId) -> Result<Option<GroupMetadata>> {
        let key_type = StorageKeyType::GroupMetadata.as_str();
        match self.storage.load(key_type, group_id.as_str())? {
            Some(data) => {
                let mut metadata: GroupMetadata = serde_json::from_slice(&data)
                    .map_err(|e| MlsError::Deserialization(e.to_string()))?;
                // Migrate legacy "role:*" keys from `custom` into `roles`
                if metadata.roles.is_empty()
                    && metadata
                        .custom
                        .keys()
                        .any(|k| k.starts_with(GroupMetadata::LEGACY_ROLE_KEY_PREFIX))
                {
                    metadata.migrate_legacy_roles();
                    // Persist the migration so it only runs once
                    if let Err(e) = self.save_group_metadata(group_id, &metadata) {
                        warn!(group_id = %group_id.as_str(), error = %e, "Failed to persist legacy role migration");
                    }
                }
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Saves group metadata to storage.
    fn save_group_metadata(&self, group_id: &GroupId, metadata: &GroupMetadata) -> Result<()> {
        let key_type = StorageKeyType::GroupMetadata.as_str();
        let data =
            serde_json::to_vec(metadata).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage.store(key_type, group_id.as_str(), &data)?;
        Ok(())
    }
}

// ============================================================================
// KEY ROTATION AND KEY PACKAGE MANAGEMENT
// ============================================================================

impl MlsManager {
    /// Updates the cryptographic keys for a group (triggers MLS self-update).
    ///
    /// This provides post-compromise security by rotating keys. The returned
    /// commit message must be sent to all other group members.
    ///
    /// # Returns
    ///
    /// Returns the commit message that must be distributed to all group members.
    pub fn update_keys(&self, group_id: &GroupId) -> Result<EncryptedMessage> {
        use openmls::treesync::LeafNodeParameters;

        let mut group = self
            .group_manager
            .load_group(group_id)?
            .ok_or_else(|| MlsError::GroupNotFound(group_id.to_string()))?;

        let signature_keys = self.get_signer()?;

        let bundle = group
            .self_update(
                &self.provider,
                &signature_keys,
                LeafNodeParameters::default(),
            )
            .map_err(|e| MlsError::OpenMls(format!("Self-update failed: {}", e)))?;

        let (commit, _welcome, _group_info) = bundle.into_contents();

        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsError::OpenMls(format!("Failed to merge self-update commit: {}", e)))?;

        self.group_manager.save_group(group_id, &group)?;

        // Update metadata last activity
        if let Some(mut metadata) = self.load_group_metadata(group_id)? {
            metadata.touch();
            self.save_group_metadata(group_id, &metadata)?;
        }

        let ciphertext = commit
            .tls_serialize_detached()
            .map_err(|e| MlsError::Serialization(e.to_string()))?;

        debug!(group_id = %group_id, epoch = %group.epoch().as_u64(), "Updated group keys");

        Ok(EncryptedMessage {
            group_id: group_id.clone(),
            message_type: MlsMessageType::Commit,
            epoch: group.epoch().as_u64(),
            ciphertext,
            sender_id: self.user_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        })
    }

    /// Ensures at least `min` valid key packages are available.
    ///
    /// Generates new key packages if the current count is below the minimum.
    /// This is useful for offline scenarios where multiple key packages should
    /// be pre-generated for distribution.
    ///
    /// # Arguments
    ///
    /// * `min` - Minimum number of key packages to maintain, capped at
    ///   [`MAX_PUSH_KEY_PACKAGES`]. The packages this mints are unclaimed, so
    ///   [`Self::take_push_key_package`] draws them down one per peer; asking
    ///   for more than the pool ceiling would only mint key material that
    ///   ceiling stops it ever handing out, while holding the pool at capacity
    ///   so every peer past the last claim is advertised a shared package.
    ///
    /// # Returns
    ///
    /// Returns the total number of valid key packages after ensuring minimum.
    pub fn ensure_min_key_packages(&self, min: usize) -> Result<usize> {
        let min = min.min(MAX_PUSH_KEY_PACKAGES);
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        // Count valid (non-expired) packages
        let mut valid_count = 0;
        for package_id in &package_ids {
            if self.load_stored_key_package(package_id)?.is_some() {
                valid_count += 1;
            }
        }

        // Generate more if needed
        let to_generate = min.saturating_sub(valid_count);
        for _ in 0..to_generate {
            self.generate_key_package()?;
            valid_count += 1;
        }

        debug!(
            valid_count = valid_count,
            generated = to_generate,
            "Ensured minimum key packages"
        );

        Ok(valid_count)
    }

    /// Returns the number of valid (non-expired) key packages available.
    pub fn count_valid_key_packages(&self) -> Result<usize> {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let package_ids = self.storage.list_keys(key_type)?;

        let mut count = 0;
        for package_id in package_ids {
            if self.load_stored_key_package(&package_id)?.is_some() {
                count += 1;
            }
        }

        Ok(count)
    }

    // ========================================================================
    // IDENTITY AND SIGNING OPERATIONS
    // ========================================================================

    /// Returns the identity public key as raw bytes.
    ///
    /// This is the Ed25519 public key used for MLS operations. It can be shared
    /// with others to establish your identity and verify signatures.
    pub fn get_identity_public_key(&self) -> Result<Vec<u8>> {
        let credential = self.get_credential()?;
        Ok(credential.signature_key.as_slice().to_vec())
    }

    /// Signs arbitrary data with the identity private key.
    ///
    /// Uses Ed25519 signatures (the same algorithm used for MLS operations).
    /// The signature can be verified by anyone with the corresponding public key.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to sign
    ///
    /// # Returns
    ///
    /// Returns the signature as raw bytes.
    pub fn sign_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use openmls_traits::signatures::Signer;

        let signer = self.get_signer()?;
        let signature = signer
            .sign(data)
            .map_err(|e| MlsError::Signing(format!("Failed to sign data: {:?}", e)))?;
        Ok(signature.as_slice().to_vec())
    }

    /// Verifies a signature against a public key.
    ///
    /// # Arguments
    ///
    /// * `public_key` - The Ed25519 public key bytes (32 bytes)
    /// * `data` - The original data that was signed
    /// * `signature` - The signature to verify (64 bytes)
    ///
    /// # Returns
    ///
    /// Returns `true` if the signature is valid, `false` otherwise.
    pub fn verify_signature(public_key: &[u8], data: &[u8], signature: &[u8]) -> Result<bool> {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        // Parse the public key (Ed25519 public keys are 32 bytes)
        let verifying_key = VerifyingKey::try_from(public_key).map_err(|e| {
            MlsError::InvalidPublicKey(format!("Invalid Ed25519 public key: {}", e))
        })?;

        // Parse the signature (Ed25519 signatures are 64 bytes)
        let sig = Signature::try_from(signature).map_err(|e| {
            MlsError::VerificationFailed(format!("Invalid signature format: {}", e))
        })?;

        // Verify the signature
        match verifying_key.verify(data, &sig) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Derives a deterministic user ID from a public key.
    ///
    /// The user ID is derived by taking the SHA-256 hash of the public key
    /// and encoding the first 20 bytes as base58. This produces a short,
    /// human-readable identifier that is collision-resistant.
    ///
    /// # Arguments
    ///
    /// * `public_key` - The Ed25519 public key bytes
    ///
    /// # Returns
    ///
    /// Returns a base58-encoded string derived from the public key.
    pub fn derive_user_id_from_public_key(public_key: &[u8]) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(public_key);
        let hash = hasher.finalize();

        // Take first 20 bytes and encode as base58
        bs58::encode(&hash[..20]).into_string()
    }
}

/// Returns `true` when `bytes` parse as a well-formed MLS wire message
/// (`MlsMessageIn` TLS framing, consuming the input exactly).
///
/// Inbound routing uses this to distinguish MLS ciphertext from legacy
/// plaintext that merely happens to be valid base64: the strict TLS framing
/// (protocol version, wire format, exact-length body) makes an accidental
/// match against non-MLS bytes vanishingly unlikely. This is a framing
/// check only — it says nothing about whether the message can be decrypted.
pub fn is_mls_framed(bytes: &[u8]) -> bool {
    MlsMessageIn::tls_deserialize_exact(bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;

    fn create_test_manager(user_id: &str) -> MlsManager {
        let storage = Arc::new(InMemoryStorage::new());
        MlsManager::new(user_id, storage).unwrap()
    }

    /// Same, but keeps a handle on the storage so a test can age a package in
    /// place — the only way to reach the expiry and purge paths without either
    /// waiting 30 days or making the clock injectable.
    fn create_test_manager_with_storage(user_id: &str) -> (MlsManager, Arc<InMemoryStorage>) {
        let storage = Arc::new(InMemoryStorage::new());
        let manager = MlsManager::new(user_id, storage.clone()).unwrap();
        (manager, storage)
    }

    /// Rewrites a stored package's expiry to `secs_ago` seconds in the past.
    fn expire_package(storage: &InMemoryStorage, package_id: &str, secs_ago: u64) {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let data = storage
            .load(key_type, package_id)
            .unwrap()
            .expect("package to age is not in storage");
        let mut bundle: KeyPackageBundle = serde_json::from_slice(&data).unwrap();
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        bundle.expires_at_ms = now_ms - (secs_ago * 1000);
        storage
            .store(key_type, package_id, &serde_json::to_vec(&bundle).unwrap())
            .unwrap();
    }

    /// Whether the private init key for `bundle` is still in provider storage.
    fn init_key_present(manager: &MlsManager, bundle: &KeyPackageBundle) -> bool {
        manager.is_key_package_usable(&bundle.key_package_data)
    }

    /// Rewrites a stored package's record without its cached provider hash
    /// ref, the way a build predating the cache would have written it.
    fn strip_cached_ref(storage: &InMemoryStorage, package_id: &str) {
        let key_type = StorageKeyType::KeyPackage.as_str();
        let data = storage
            .load(key_type, package_id)
            .unwrap()
            .expect("package to strip is not in storage");
        let mut bundle: KeyPackageBundle = serde_json::from_slice(&data).unwrap();
        bundle.provider_hash_ref = None;
        storage
            .store(key_type, package_id, &serde_json::to_vec(&bundle).unwrap())
            .unwrap();
    }

    /// The provider hash ref is stamped at mint so the pool scan never pays a
    /// TLS parse plus a signature validation per stored package per push —
    /// unstamped, a scan-heavy path (many peers, full pool) costs tens of
    /// debug-build seconds.
    #[test]
    fn test_minted_package_carries_its_provider_hash_ref() {
        let manager = create_test_manager("alice");
        let bundle = manager.take_push_key_package("bob").unwrap().bundle;

        let cached = bundle
            .provider_hash_ref
            .as_deref()
            .expect("mint must stamp the provider hash ref");
        let derived = manager
            .key_package_hash_ref(&bundle.key_package_data)
            .expect("freshly minted package bytes must parse")
            .tls_serialize_detached()
            .unwrap();
        assert_eq!(
            cached,
            &derived[..],
            "cached ref disagrees with the one derived from the package bytes"
        );
    }

    /// Records written before the cache existed take the derive path once and
    /// are stamped back, so every later load is cheap.
    #[test]
    fn test_record_without_cached_ref_is_backfilled_on_first_load() {
        let (manager, storage) = create_test_manager_with_storage("alice");
        let minted = manager.take_push_key_package("bob").unwrap().bundle;
        strip_cached_ref(&storage, &minted.package_id);

        let loaded = manager
            .key_package_by_id(&minted.package_id)
            .unwrap()
            .expect("a record without a cached ref must still load");
        assert_eq!(
            loaded.provider_hash_ref, minted.provider_hash_ref,
            "load must hand back a bundle stamped with the same ref the mint computed"
        );

        let stored: KeyPackageBundle = serde_json::from_slice(
            &storage
                .load(StorageKeyType::KeyPackage.as_str(), &minted.package_id)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            stored.provider_hash_ref, minted.provider_hash_ref,
            "the backfilled ref must be persisted, not only returned"
        );
    }

    /// The purge destroys key material for pre-cache records too, deriving
    /// the ref they never carried.
    #[test]
    fn test_purge_destroys_init_key_for_a_record_without_cached_ref() {
        let (manager, storage) = create_test_manager_with_storage("alice");
        let bundle = manager.take_push_key_package("bob").unwrap().bundle;
        strip_cached_ref(&storage, &bundle.package_id);
        expire_package(
            &storage,
            &bundle.package_id,
            KEY_PACKAGE_PURGE_GRACE_SECS + 60,
        );

        assert!(manager
            .key_package_by_id(&bundle.package_id)
            .unwrap()
            .is_none());
        assert!(
            !init_key_present(&manager, &bundle),
            "the private init key outlived a pre-cache package"
        );
    }

    /// An expired package stops being advertised immediately, but its init key
    /// has to outlive the last advertisement: a peer handed it just before
    /// expiry may still Welcome us, and that Welcome is only processable while
    /// the key is resident.
    #[test]
    fn test_expired_package_is_withdrawn_but_its_init_key_survives_the_grace_window() {
        let (manager, storage) = create_test_manager_with_storage("alice");
        let bundle = manager.take_push_key_package("bob").unwrap().bundle;

        expire_package(&storage, &bundle.package_id, 60);

        assert!(
            manager
                .key_package_by_id(&bundle.package_id)
                .unwrap()
                .is_none(),
            "an expired package must not be handed out"
        );
        assert!(
            init_key_present(&manager, &bundle),
            "the init key was destroyed on the expiry edge, breaking a late Welcome"
        );

        // The next push to that peer mints a replacement rather than reviving it.
        let replacement = manager.take_push_key_package("bob").unwrap().bundle;
        assert_ne!(replacement.package_id, bundle.package_id);
    }

    /// Past the grace window the key material is genuinely destroyed. Without
    /// this, every package this device ever minted keeps its init key for the
    /// life of the install — deleting the bundle record, which is all this
    /// crate used to do, never touched the key OpenMLS holds.
    #[test]
    fn test_package_past_the_grace_window_has_its_init_key_destroyed() {
        let (manager, storage) = create_test_manager_with_storage("alice");
        let bundle = manager.take_push_key_package("bob").unwrap().bundle;
        assert!(init_key_present(&manager, &bundle));

        expire_package(
            &storage,
            &bundle.package_id,
            KEY_PACKAGE_PURGE_GRACE_SECS + 60,
        );

        // The purge runs as a side effect of the scan that reads it.
        assert!(manager
            .key_package_by_id(&bundle.package_id)
            .unwrap()
            .is_none());

        assert!(
            !init_key_present(&manager, &bundle),
            "the private init key outlived its package"
        );
        assert!(
            storage
                .load(StorageKeyType::KeyPackage.as_str(), &bundle.package_id)
                .unwrap()
                .is_none(),
            "the bundle record outlived its key material"
        );
    }

    /// Reclaiming an unpublished package has to reclaim the key too, for the
    /// same reason — the publication path mints one per failed refresh, and a
    /// record-only delete would strand fresh key material every time.
    #[test]
    fn test_delete_key_package_destroys_the_init_key() {
        let manager = create_test_manager("alice");
        let bundle = manager.generate_publication_key_package().unwrap();
        assert!(init_key_present(&manager, &bundle));

        manager.delete_key_package(&bundle.package_id).unwrap();

        assert!(
            !init_key_present(&manager, &bundle),
            "delete_key_package left the private init key in provider storage"
        );
    }

    /// A legacy record — raw key package bytes rather than a serialized bundle,
    /// which `load_stored_key_package` still upgrades in place — must have its
    /// init key destroyed too. Reading the record as unparseable and deleting
    /// only the record is what strands key material.
    #[test]
    fn test_delete_key_package_destroys_the_init_key_of_a_legacy_record() {
        let (manager, storage) = create_test_manager_with_storage("alice");
        let bundle = manager.generate_publication_key_package().unwrap();
        // Rewrite the record the way a build predating the bundle wrote it.
        storage
            .store(
                StorageKeyType::KeyPackage.as_str(),
                &bundle.package_id,
                &bundle.key_package_data,
            )
            .unwrap();

        manager.delete_key_package(&bundle.package_id).unwrap();

        assert!(
            !init_key_present(&manager, &bundle),
            "a legacy record's private init key survived its deletion"
        );
        assert!(
            storage
                .load(StorageKeyType::KeyPackage.as_str(), &bundle.package_id)
                .unwrap()
                .is_none(),
            "the legacy record outlived its key material"
        );
    }

    /// A record whose bytes name no key package at all still gets deleted —
    /// there is no material to destroy, and leaving it would make the pool scan
    /// carry it forever.
    #[test]
    fn test_delete_key_package_removes_an_unreadable_record() {
        let (manager, storage) = create_test_manager_with_storage("alice");
        storage
            .store(
                StorageKeyType::KeyPackage.as_str(),
                "junk",
                b"not a package",
            )
            .unwrap();

        manager.delete_key_package("junk").unwrap();

        assert!(storage
            .load(StorageKeyType::KeyPackage.as_str(), "junk")
            .unwrap()
            .is_none());
    }

    /// `ensure_min_key_packages` mints *unclaimed* packages, which the push
    /// path draws down one per peer. Minting past the pool ceiling would put
    /// material in storage that the ceiling then stops it ever handing out,
    /// while pinning the pool at capacity so every peer past the last claim is
    /// advertised a shared init key.
    #[test]
    fn test_ensure_min_key_packages_is_capped_at_the_pool_ceiling() {
        let manager = create_test_manager("alice");

        let count = manager
            .ensure_min_key_packages(MAX_PUSH_KEY_PACKAGES + 50)
            .unwrap();

        assert_eq!(count, MAX_PUSH_KEY_PACKAGES);
        assert_eq!(
            manager.count_valid_key_packages().unwrap(),
            MAX_PUSH_KEY_PACKAGES,
            "pre-filling minted past the ceiling the push path enforces"
        );
    }

    #[test]
    fn test_manager_creation() {
        let manager = create_test_manager("alice");
        assert_eq!(manager.user_id(), "alice");
    }

    #[test]
    fn test_key_package_generation() {
        let manager = create_test_manager("alice");
        let package = manager.generate_key_package().unwrap();

        assert_eq!(package.user_id, "alice");
        assert!(!package.key_package_data.is_empty());
        assert!(!package.is_expired());
    }

    /// The push path must never hand out a package standing in a published
    /// slot. If it did, the pushed-to peer and a stranger who fetched the
    /// record would race for the same single-use init key, and the loser's
    /// Welcome is unprocessable.
    #[test]
    fn test_get_or_create_never_returns_a_publication_package() {
        let manager = create_test_manager("alice");
        let reserved = manager.generate_publication_key_package().unwrap();
        assert!(reserved.reserved_for_publication);

        let pushed = manager.get_or_create_key_package().unwrap();
        assert_ne!(
            pushed.package_id, reserved.package_id,
            "the push path took the package a published record is standing on"
        );
        assert!(!pushed.reserved_for_publication);

        // And it stays stable across calls — the reserved one is skipped every
        // time, not just minted around once.
        let pushed_again = manager.get_or_create_key_package().unwrap();
        assert_eq!(pushed.package_id, pushed_again.package_id);
    }

    /// `key_package_by_id` returning `None` is the consumption signal the
    /// publication slots run on, so it must distinguish a package this device
    /// still holds from one it does not.
    #[test]
    fn test_key_package_by_id_reports_presence() {
        let manager = create_test_manager("alice");
        let bundle = manager.generate_publication_key_package().unwrap();

        let found = manager.key_package_by_id(&bundle.package_id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().package_id, bundle.package_id);

        assert!(manager
            .key_package_by_id("no-such-package")
            .unwrap()
            .is_none());
    }

    /// Publication slots are independent: two slots must never end up holding
    /// the same package, or one Welcome consumes both.
    #[test]
    fn test_publication_packages_are_distinct_per_slot() {
        let manager = create_test_manager("alice");
        let a = manager.generate_publication_key_package().unwrap();
        let b = manager.generate_publication_key_package().unwrap();
        assert_ne!(a.package_id, b.package_id);
        assert_ne!(a.key_package_data, b.key_package_data);
    }

    #[test]
    fn test_get_or_create_key_package() {
        let manager = create_test_manager("alice");
        let pkg1 = manager.get_or_create_key_package().unwrap();
        let pkg2 = manager.get_or_create_key_package().unwrap();
        // Since we are now properly persisting keys, pkg2 should be the same as pkg1
        // IF the logic reuses existing key packages.
        // get_or_create_key_package logic iterates list_keys.
        assert_eq!(pkg1.package_id, pkg2.package_id);
    }

    // ========================================================================
    // PUSH-PATH KEY PACKAGE POOL
    // ========================================================================

    /// The property the whole per-peer pool exists for: two peers must never be
    /// advertised the same init key. Sharing one weakens forward secrecy at
    /// session establishment (a single compromised init key opens every Welcome
    /// built against it) and is the LastResort-style reuse RFC 9420 §16.8
    /// permits only as a denial-of-service fallback.
    #[test]
    fn test_push_path_gives_each_peer_its_own_key_package() {
        let manager = create_test_manager("alice");

        let for_bob = manager.take_push_key_package("bob").unwrap();
        let for_carol = manager.take_push_key_package("carol").unwrap();

        assert_ne!(
            for_bob.bundle.package_id, for_carol.bundle.package_id,
            "two peers were advertised the same key package"
        );
        assert_ne!(
            for_bob.bundle.key_package_data, for_carol.bundle.key_package_data,
            "two peers were advertised the same init key"
        );
        assert!(!for_bob.pool_exhausted && !for_carol.pool_exhausted);
        assert_eq!(for_bob.bundle.assigned_peer.as_deref(), Some("bob"));
        assert_eq!(for_carol.bundle.assigned_peer.as_deref(), Some("carol"));
    }

    /// The other half of the property: a repeat push to one peer must NOT mint
    /// fresh key material. Pushes to the same peer are routine (rediscovery,
    /// session establishment, group invites), and minting per push would grow
    /// the pool without bound and hit the ceiling within days.
    #[test]
    fn test_push_path_reuses_one_package_for_repeat_pushes_to_a_peer() {
        let manager = create_test_manager("alice");

        let first = manager.take_push_key_package("bob").unwrap();
        let second = manager.take_push_key_package("bob").unwrap();
        let third = manager.take_push_key_package("bob").unwrap();

        assert_eq!(first.bundle.package_id, second.bundle.package_id);
        assert_eq!(first.bundle.package_id, third.bundle.package_id);
        assert_eq!(
            manager.count_valid_key_packages().unwrap(),
            1,
            "repeat pushes to one peer minted extra key material"
        );
    }

    /// A peer's package is rotated by *consumption*, not by time: once a
    /// Welcome built against it has been processed the init key is gone, and
    /// the next push to that peer must mint a fresh one rather than re-handing
    /// a package nobody can use.
    #[test]
    fn test_push_path_mints_a_fresh_package_after_the_peer_consumed_one() {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");

        let advertised = alice.take_push_key_package("bob").unwrap().bundle;
        bob.import_key_package(
            "alice",
            &advertised.key_package_data,
            KeyPackageTrust::FirstUse,
        )
        .unwrap();
        let welcome = bob.create_session("alice").unwrap();
        alice.join_session(&welcome).unwrap();

        let next = alice.take_push_key_package("bob").unwrap();
        assert_ne!(
            next.bundle.package_id, advertised.package_id,
            "a consumed package was advertised again"
        );
        assert!(!next.pool_exhausted);
    }

    /// The race the old shape created, end to end: one package handed to two
    /// peers means the second Welcome is built against an init key the first
    /// has already consumed, and it can never be processed. Nothing re-drives
    /// the exchange, so that peer is stuck.
    #[test]
    fn test_two_peers_can_both_establish_sessions_after_being_pushed_to() {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let carol = create_test_manager("carol");

        for (peer, manager) in [("bob", &bob), ("carol", &carol)] {
            let advertised = alice.take_push_key_package(peer).unwrap().bundle;
            manager
                .import_key_package(
                    "alice",
                    &advertised.key_package_data,
                    KeyPackageTrust::FirstUse,
                )
                .unwrap();
        }

        // Both Welcomes are built before either is processed — the interleaving
        // that makes shared init keys fail.
        let from_bob = bob.create_session("alice").unwrap();
        let from_carol = carol.create_session("alice").unwrap();

        alice.join_session(&from_bob).unwrap();
        alice
            .join_session(&from_carol)
            .expect("second peer's Welcome must still be processable");

        assert!(alice.has_session("bob").unwrap());
        assert!(alice.has_session("carol").unwrap());
    }

    /// Publication slots stay off-limits to the push path for the same
    /// single-use reason, now via the peer-keyed entry point.
    #[test]
    fn test_push_path_never_takes_a_publication_package() {
        let manager = create_test_manager("alice");
        let reserved = manager.generate_publication_key_package().unwrap();

        for peer in ["bob", "carol", "dave"] {
            let taken = manager.take_push_key_package(peer).unwrap();
            assert_ne!(
                taken.bundle.package_id, reserved.package_id,
                "the push path took the package a published record stands on"
            );
        }
    }

    /// The peer-less entry point (the FFI surface and tests) must not hand out
    /// a package a peer is expected to use, or it re-opens the cross-peer
    /// sharing the pool prevents.
    #[test]
    fn test_get_or_create_never_returns_a_peer_assigned_package() {
        let manager = create_test_manager("alice");
        let for_bob = manager.take_push_key_package("bob").unwrap().bundle;

        let unclaimed = manager.get_or_create_key_package().unwrap();
        assert_ne!(unclaimed.package_id, for_bob.package_id);
        assert!(unclaimed.assigned_peer.is_none());
    }

    /// An unclaimed package — minted by the peer-less entry point, or by a
    /// build predating assignment — is claimed rather than left behind, so
    /// upgrading does not strand the package already in storage.
    #[test]
    fn test_push_path_claims_an_unclaimed_package_instead_of_stranding_it() {
        let manager = create_test_manager("alice");
        let legacy = manager.get_or_create_key_package().unwrap();
        assert!(legacy.assigned_peer.is_none());

        let taken = manager.take_push_key_package("bob").unwrap();
        assert_eq!(
            taken.bundle.package_id, legacy.package_id,
            "the pre-existing package was stranded instead of claimed"
        );
        assert_eq!(taken.bundle.assigned_peer.as_deref(), Some("bob"));
        assert_eq!(manager.count_valid_key_packages().unwrap(), 1);
    }

    /// At the ceiling the pool stops growing and shares a package, which is the
    /// old behaviour — deliberately, since refusing to advertise would cost
    /// session establishment. It must say so, because that is the one condition
    /// under which the reuse this pool removes is back.
    #[test]
    fn test_push_pool_ceiling_shares_a_package_and_reports_it() {
        let manager = create_test_manager("alice");

        for i in 0..MAX_PUSH_KEY_PACKAGES {
            let taken = manager.take_push_key_package(&format!("peer{i}")).unwrap();
            assert!(
                !taken.pool_exhausted,
                "pool reported exhausted at {i}, below the ceiling"
            );
        }
        assert_eq!(
            manager.count_valid_key_packages().unwrap(),
            MAX_PUSH_KEY_PACKAGES
        );

        let over = manager.take_push_key_package("one-peer-too-many").unwrap();
        assert!(over.pool_exhausted, "ceiling breach was not reported");
        assert_eq!(
            manager.count_valid_key_packages().unwrap(),
            MAX_PUSH_KEY_PACKAGES,
            "the pool grew past its ceiling"
        );

        // A peer that already holds a package keeps getting its own, even with
        // the pool at capacity — the ceiling must not downgrade existing peers.
        let established = manager.take_push_key_package("peer0").unwrap();
        assert!(!established.pool_exhausted);
        assert_eq!(established.bundle.assigned_peer.as_deref(), Some("peer0"));
    }

    /// The ceiling bounds how many packages *exist*, so it must gate minting
    /// and nothing else. A full pool holding an unclaimed package has one to
    /// give: claiming it relabels a package rather than adding one, and
    /// degrading to a shared init key while it sat idle would weaken forward
    /// secrecy to stay under a bound the claim never approaches.
    #[test]
    fn test_ceiling_claims_an_unclaimed_package_rather_than_sharing() {
        let manager = create_test_manager("alice");

        for i in 0..(MAX_PUSH_KEY_PACKAGES - 1) {
            manager.take_push_key_package(&format!("peer{i}")).unwrap();
        }
        // The last live slot is an unclaimed package — as the peer-less entry
        // point, `ensure_min_key_packages`, or a pre-upgrade record leaves one.
        let unclaimed = manager.get_or_create_key_package().unwrap();
        assert_eq!(
            manager.count_valid_key_packages().unwrap(),
            MAX_PUSH_KEY_PACKAGES,
            "test premise: the pool is at the ceiling"
        );

        let taken = manager.take_push_key_package("late-arrival").unwrap();
        assert!(
            !taken.pool_exhausted,
            "shared a package while a claimable one was in the pool"
        );
        assert_eq!(
            taken.bundle.package_id, unclaimed.package_id,
            "the claimable package was passed over"
        );
        assert_eq!(taken.bundle.assigned_peer.as_deref(), Some("late-arrival"));
        assert_eq!(
            manager.count_valid_key_packages().unwrap(),
            MAX_PUSH_KEY_PACKAGES,
            "claiming grew the pool"
        );

        // With the last unclaimed package now spoken for, the next new peer has
        // nothing left to claim and does take the shared-package degradation.
        let over = manager.take_push_key_package("one-peer-too-many").unwrap();
        assert!(over.pool_exhausted);
    }

    #[test]
    fn test_import_key_package_binds_credential_identity() {
        let alice = create_test_manager("alice");
        let mallory = create_test_manager("mallory");
        let mallory_kp = mallory.generate_key_package().unwrap();

        // Importing mallory's key package under "bob" must be rejected —
        // otherwise a session "with bob" would encrypt to mallory's keys.
        let err = alice
            .import_key_package(
                "bob",
                &mallory_kp.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .unwrap_err();
        assert!(matches!(err, MlsError::CredentialIdentityMismatch { .. }));

        // Importing under the matching identity succeeds.
        alice
            .import_key_package(
                "mallory",
                &mallory_kp.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .unwrap();
    }

    /// A "bob" that is not the real bob: a separate MLS identity whose basic
    /// credential nevertheless says `bob`. This is what an attacker who can
    /// write the key-package cache produces — nothing stops them, because a
    /// basic credential is per RFC 9420 "a bare assertion of an identity".
    fn impostor_claiming(user_id: &str) -> MlsManager {
        create_test_manager(user_id)
    }

    #[test]
    fn test_credential_identity_check_alone_does_not_stop_a_substitution() {
        // The premise for the pin check below, stated as a test so it cannot
        // quietly stop being true. `verify_credential_identity` compares a
        // self-asserted string, so an impostor who writes `bob` into their own
        // credential sails through it.
        let alice = create_test_manager("alice");
        let impostor = impostor_claiming("bob");
        let impostor_kp = impostor.generate_key_package().unwrap();

        alice
            .import_key_package(
                "bob",
                &impostor_kp.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .expect("the identity check cannot tell the two bobs apart");
    }

    #[test]
    fn test_substituted_key_package_is_refused_against_the_pin() {
        // The same substitution, refused, because the pin compares the leaf
        // signature key — the peer's MLS identity key, which is exactly the
        // value the control-message gate verified a signature against.
        let alice = create_test_manager("alice");
        let real_bob = create_test_manager("bob");
        let impostor = impostor_claiming("bob");

        let pin = real_bob.get_identity_public_key().unwrap();
        let impostor_kp = impostor.generate_key_package().unwrap();
        let real_kp = real_bob.generate_key_package().unwrap();

        let err = alice
            .import_key_package(
                "bob",
                &impostor_kp.key_package_data,
                KeyPackageTrust::Pinned(&pin),
            )
            .unwrap_err();
        assert!(
            matches!(err, MlsError::KeyPackagePinMismatch { ref peer_id } if peer_id == "bob"),
            "expected a pin mismatch, got {:?}",
            err
        );

        // The genuine article still imports.
        alice
            .import_key_package(
                "bob",
                &real_kp.key_package_data,
                KeyPackageTrust::Pinned(&pin),
            )
            .expect("the real peer's key package matches its own pin");
    }

    #[test]
    fn test_empty_pin_is_not_treated_as_no_pin() {
        // An empty pin would compare against nothing and accept everything,
        // which is worse than declining to check — so it is refused outright
        // rather than silently degrading to first-use.
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();

        let err = alice
            .import_key_package(
                "bob",
                &bob_kp.key_package_data,
                KeyPackageTrust::Pinned(&[]),
            )
            .unwrap_err();
        assert!(matches!(err, MlsError::KeyPackagePinMismatch { .. }));
    }

    #[test]
    fn test_add_group_member_binds_identity_and_pin() {
        // The group path ran neither check before — not even the credential
        // identity one — so a package could join a group under a roster label
        // that had nothing to do with it.
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let mallory = create_test_manager("mallory");
        let impostor = impostor_claiming("bob");

        let group_id = alice.create_group("Test Group").unwrap().group_id;

        // Wrong identity outright.
        let mallory_kp = mallory.generate_key_package().unwrap();
        let err = alice
            .add_group_member(
                &group_id,
                "bob",
                &mallory_kp.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .unwrap_err();
        assert!(matches!(err, MlsError::CredentialIdentityMismatch { .. }));

        // Right identity string, wrong key, and a pin to catch it.
        let pin = bob.get_identity_public_key().unwrap();
        let impostor_kp = impostor.generate_key_package().unwrap();
        let err = alice
            .add_group_member(
                &group_id,
                "bob",
                &impostor_kp.key_package_data,
                KeyPackageTrust::Pinned(&pin),
            )
            .unwrap_err();
        assert!(matches!(err, MlsError::KeyPackagePinMismatch { .. }));

        // The genuine invitee joins.
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .add_group_member(
                &group_id,
                "bob",
                &bob_kp.key_package_data,
                KeyPackageTrust::Pinned(&pin),
            )
            .expect("the real invitee is admitted");
    }

    #[test]
    fn test_import_key_package_rejects_hostile_user_id() {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();

        for hostile in ["", "..", "bob/evil", "bob\\evil", "bob\0evil"] {
            let err = alice
                .import_key_package(hostile, &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
                .unwrap_err();
            assert!(
                matches!(err, MlsError::InvalidUserId(_)),
                "expected InvalidUserId for {:?}, got {:?}",
                hostile,
                err
            );
        }
    }

    #[test]
    fn test_get_contact_key_package_rejects_poisoned_store() {
        // A key package written to storage out-of-band (bypassing import
        // validation, e.g. persisted before this fix) under the wrong user
        // id must fail identity verification at use time.
        let storage = Arc::new(InMemoryStorage::new());
        let alice = MlsManager::new("alice", storage.clone()).unwrap();
        let mallory = create_test_manager("mallory");
        let mallory_kp = mallory.generate_key_package().unwrap();

        storage
            .store(
                StorageKeyType::ContactKeyPackage.as_str(),
                "bob",
                &mallory_kp.key_package_data,
            )
            .unwrap();

        // create_session("bob") loads the poisoned package and must refuse.
        let err = alice.create_session("bob").unwrap_err();
        assert!(matches!(err, MlsError::CredentialIdentityMismatch { .. }));
    }

    #[test]
    fn test_replace_session_with_welcome_rejects_hostile_inviter_id() {
        // `inviter_id` arrives on the wire and is used as a raw storage key
        // for deletes — hostile values must be rejected before any storage
        // operation runs.
        let alice = create_test_manager("alice");
        let welcome = WelcomeMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            welcome_data: vec![],
            inviter_id: "../../etc".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };
        let err = alice.replace_session_with_welcome(&welcome).unwrap_err();
        assert!(matches!(err, MlsError::InvalidUserId(_)));
    }

    /// Builds a converged alice/bob 1:1 session for sender-binding tests.
    fn create_test_session() -> (MlsManager, MlsManager) {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
            .unwrap();
        let welcome = alice.create_session("bob").unwrap();
        bob.join_session(&welcome).unwrap();
        (alice, bob)
    }

    #[test]
    fn test_session_decrypt_rejects_spoofed_sender() {
        let (alice, bob) = create_test_session();

        // A message from alice's session must not be attributable to a
        // different wire sender. The envelope names `session:alice:bob`, which
        // is not the slot bob shares with "mallory", so the slot binding
        // rejects it *before* the group is loaded — strictly earlier than the
        // SEC-M1 credential check, which cannot run until a decrypt succeeds.
        let ct = alice.encrypt_for_user("bob", b"hello").unwrap();
        let err = bob.decrypt_from_user(&ct, "mallory").unwrap_err();
        assert!(
            matches!(err, MlsError::SessionIdentityMismatch { .. }),
            "spoofed sender must be rejected by the slot binding, got {:?}",
            err
        );

        // Rejecting pre-decrypt also means the forgery does not consume the
        // ratchet generation it named, so the *same* generation still decrypts
        // for the legitimate sender. (Before the binding, a spoofed frame
        // burned the generation and the genuine message was lost.)
        let pt = bob.decrypt_from_user(&ct, "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"hello"[..]));

        // SEC-M1 itself (correct slot, wrong credential) stays reachable and is
        // covered by `test_group_decrypt_rejects_spoofed_sender`, where a group
        // has enough members for the two identities to differ.
    }

    #[test]
    fn test_epoch_fork_classifies_as_session_desync() {
        // An established session that forks (one side advances its epoch without
        // the other merging the commit) must surface as the *recoverable*
        // `SessionDesync`, not an opaque `Decryption` — that is what lets the
        // protocol layer re-key instead of silently dropping.
        let (alice, bob) = create_test_session();

        // Alice self-updates and merges locally, advancing only her epoch; bob
        // never sees the commit, so the two sides are now one epoch apart.
        let group_id = GroupId::new("session:alice:bob").unwrap();
        alice.update_keys(&group_id).unwrap();

        // A message alice encrypts at her new epoch cannot decrypt at bob's old
        // epoch → OpenMLS `WrongEpoch` → `MlsError::SessionDesync`.
        let ct = alice.encrypt_for_user("bob", b"after-fork").unwrap();
        let err = bob.decrypt_from_user(&ct, "alice").unwrap_err();
        assert!(
            matches!(err, MlsError::SessionDesync(_)),
            "epoch fork must classify as SessionDesync, got {:?}",
            err
        );
    }

    /// Serializes an `MlsMessage(PrivateMessage)` from scratch — no group, no
    /// keys, no captured ciphertext. Wire format per RFC 9420:
    /// `version(u16) || wire_format(u16) || group_id<V> || epoch(u64) ||
    /// content_type(u8) || authenticated_data<V> || encrypted_sender_data<V> ||
    /// ciphertext<V>`, where `<V>` is a QUIC varint length (single byte below
    /// 64). Used to demonstrate what an off-path attacker can actually build.
    fn forge_private_message(group_id: &str, epoch: u64) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_be_bytes()); // ProtocolVersion::Mls10
        out.extend_from_slice(&2u16.to_be_bytes()); // WireFormat::PrivateMessage
        assert!(group_id.len() < 64, "test group ids stay in 1-byte varints");
        out.push(group_id.len() as u8);
        out.extend_from_slice(group_id.as_bytes());
        out.extend_from_slice(&epoch.to_be_bytes());
        out.push(1); // ContentType::Application
        out.push(0); // authenticated_data: empty
        out.push(8); // encrypted_sender_data: 8 bytes of nothing
        out.extend_from_slice(&[0u8; 8]);
        out.push(16); // ciphertext: 16 bytes of nothing
        out.extend_from_slice(&[0u8; 16]);
        out
    }

    #[test]
    fn test_forged_frame_reaches_session_desync_without_any_key_material() {
        // Honest statement of the residual threat model. OpenMLS validates the
        // framing header (group id, then epoch) *before* any AEAD, sender-data
        // decryption, or signature check, and a 1:1 slot id is derivable from
        // two public user ids. So an attacker with no key material, no captured
        // ciphertext and no session can hand-serialize a frame that classifies
        // as the recoverable `SessionDesync` — the classification that drives a
        // re-key. This is inherent to MLS framing, not an OpenMLS bug.
        //
        // The mitigation is NOT that this is unreachable (it is reachable, as
        // asserted here) but that everything hung off it is bounded and
        // non-destructive: the slot binding below, the per-peer re-key floor,
        // and a heal that no longer discards queued plaintext.
        let (_alice, bob) = create_test_session();

        let forged = EncryptedMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            message_type: MlsMessageType::Application,
            epoch: 9_999,
            ciphertext: forge_private_message("session:alice:bob", 9_999),
            sender_id: "alice".to_string(),
            timestamp_ms: 0,
        };

        let err = bob.decrypt_from_user(&forged, "alice").unwrap_err();
        assert!(
            matches!(err, MlsError::SessionDesync(_)),
            "a forged future-epoch frame reaches the desync classification pre-AEAD, got {:?}",
            err
        );
    }

    #[test]
    fn test_forged_frame_naming_a_foreign_slot_is_rejected_before_desync() {
        // The slot binding is what stops the forgery above from being pointed
        // at *any* session while naming *any* sender. Without it the claimed
        // sender and the targeted group are independent, so one derivable slot
        // id yields a re-key (and a peer-keyed map entry, and a key-package
        // send) for arbitrary attacker-chosen identities.
        let (_alice, bob) = create_test_session();

        let forged = EncryptedMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            message_type: MlsMessageType::Application,
            epoch: 9_999,
            ciphertext: forge_private_message("session:alice:bob", 9_999),
            sender_id: "mallory".to_string(),
            timestamp_ms: 0,
        };

        let err = bob.decrypt_from_user(&forged, "mallory").unwrap_err();
        assert!(
            matches!(err, MlsError::SessionIdentityMismatch { .. }),
            "a frame naming another pair's slot must not classify as recoverable, got {:?}",
            err
        );
    }

    #[test]
    fn test_corrupt_ciphertext_is_not_classified_as_session_desync() {
        // A malformed frame must stay a plain decrypt failure, which the
        // protocol layer fails closed on. Note this covers only *malformed*
        // input — it never reaches the framing validation, so it is not
        // evidence that forged frames cannot reach `SessionDesync`. They can:
        // see `test_forged_frame_reaches_session_desync_without_any_key_material`.
        let (alice, bob) = create_test_session();
        let mut ct = alice.encrypt_for_user("bob", b"hello").unwrap();
        // Corrupt the AEAD-protected body.
        for b in ct.ciphertext.iter_mut() {
            *b ^= 0xFF;
        }
        let err = bob.decrypt_from_user(&ct, "alice").unwrap_err();
        assert!(
            !matches!(err, MlsError::SessionDesync(_)),
            "corrupt ciphertext must not be treated as a recoverable desync, got {:?}",
            err
        );
    }

    #[test]
    fn test_join_session_binds_slot_to_inviter() {
        // Regression (session-hijack): a Welcome from an authenticated inviter
        // may only install the 1:1 session slot for (self, inviter). An inviter
        // that names a *third* party's slot must be rejected before any group
        // state is touched — otherwise the inviter overwrites/seeds the
        // victim's session with that third party, so the victim's outbound
        // messages to that party encrypt to the attacker's group.
        let bob = create_test_manager("bob");

        // mallory, authenticating honestly as herself, sends bob a Welcome whose
        // group_id squats alice+bob's session slot.
        let hijack = WelcomeMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            welcome_data: vec![], // rejected before deserialization
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };

        // Both join entry points must reject the mismatched slot.
        let err = bob.join_session(&hijack).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeIdentityMismatch { .. }),
            "join_session: expected WelcomeIdentityMismatch, got {:?}",
            err
        );
        let err = bob.replace_session_with_welcome(&hijack).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeIdentityMismatch { .. }),
            "replace_session_with_welcome: expected WelcomeIdentityMismatch, got {:?}",
            err
        );

        // Nothing was installed: bob has no session with either identity.
        assert!(!bob.has_session("alice").unwrap());
        assert!(!bob.has_session("mallory").unwrap());

        // The check is precise, not over-broad: a correctly-slotted Welcome from
        // mallory (group_id == session:bob:mallory, inviter_id == mallory) still
        // joins normally.
        let bob_kp = bob.generate_key_package().unwrap();
        let mallory = create_test_manager("mallory");
        mallory
            .import_key_package("bob", &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
            .unwrap();
        let legit = mallory.create_session("bob").unwrap();
        bob.join_session(&legit).unwrap();
        assert!(bob.has_session("mallory").unwrap());
    }

    #[test]
    fn test_join_group_rejects_session_namespace() {
        // Regression (session-hijack via the group-Welcome path): the identity
        // binding on `join_session` only guards the session-Welcome path. A
        // group Welcome carries an attacker-controllable `group_id` and writes
        // the SAME storage/OpenMLS keyspace, so `join_group` must refuse the
        // reserved `session:` namespace outright — otherwise an authenticated
        // peer could seed/overwrite a third party's 1:1 session slot and
        // hijack the victim's outbound encryption.
        let bob = create_test_manager("bob");

        let squat = WelcomeMessage {
            group_id: GroupId::new("session:alice:bob").unwrap(),
            welcome_data: vec![], // rejected before deserialization
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };

        let err = bob.join_group(&squat).unwrap_err();
        assert!(
            matches!(err, MlsError::ReservedSessionNamespace { .. }),
            "join_group: expected ReservedSessionNamespace, got {:?}",
            err
        );

        // The squatted 1:1 slot was never installed.
        assert!(!bob.has_session("alice").unwrap());

        // Precision: a legitimately-namespaced group Welcome is not caught by
        // this guard (it fails later at deserialization of the empty blob, not
        // with ReservedSessionNamespace).
        let legit_ns = WelcomeMessage {
            group_id: GroupId::new("group:0bd6e5f2-3a70-4a4e-9c3f-1c1f2a3b4c5d").unwrap(),
            welcome_data: vec![],
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };
        let err = bob.join_group(&legit_ns).unwrap_err();
        assert!(
            !matches!(err, MlsError::ReservedSessionNamespace { .. }),
            "a group:-namespaced Welcome must not trip the session-namespace guard, got {:?}",
            err
        );
    }

    #[test]
    fn test_join_group_rejects_embedded_group_id_mismatch() {
        // Regression (HIGH-1): OpenMLS persists a joined group under the group
        // id embedded in the Welcome's GroupContext — a value the inviter picks
        // freely at creation (`new_with_group_id`) — while our storage marker
        // and every load/delete lookup key off the *wire* `group_id`, which is
        // all the SEC-M5/M6 bindings validate. If the two diverge, `into_group`
        // would install the group under the attacker's embedded id: an
        // arbitrary slot the wire-id checks never inspected. The join must bind
        // embedded == wire and reject before any state is written.
        let bob = create_test_manager("bob");
        let mallory = create_test_manager("mallory");

        // Mallory builds a real group whose EMBEDDED id is one value...
        let bob_kp = bob.generate_key_package().unwrap();
        let embedded = GroupId::new("group:11111111-1111-4111-8111-111111111111").unwrap();
        let cred = mallory.get_credential().unwrap();
        let signer = mallory.get_signer().unwrap();
        mallory
            .group_manager
            .create_group(&embedded, &cred, &signer)
            .unwrap();
        let (welcome, _commit) = mallory
            .add_group_member(
                &embedded,
                "bob",
                &bob_kp.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .unwrap();
        assert_eq!(welcome.group_id, embedded);

        // ...but presents it under a DIFFERENT wire group_id. The wire id clears
        // the reserved-namespace guard (not `session:`), so only the embedded-id
        // binding stands between the attacker and an arbitrary slot.
        let wire = GroupId::new("group:22222222-2222-4222-8222-222222222222").unwrap();
        let tampered = WelcomeMessage {
            group_id: wire.clone(),
            ..welcome
        };

        let err = bob.join_group(&tampered).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeGroupIdMismatch { .. }),
            "expected WelcomeGroupIdMismatch, got {:?}",
            err
        );

        // Nothing was installed under EITHER id — the reject precedes into_group.
        assert!(bob.group_manager.load_group(&wire).unwrap().is_none());
        assert!(bob.group_manager.load_group(&embedded).unwrap().is_none());
    }

    #[test]
    fn test_welcome_embedded_id_cannot_hijack_session_slot() {
        // Regression (HIGH-1, the SEC-M6 hijack reached through the embedded id).
        // Bob has a live 1:1 session with Alice at `session:alice:bob`. Mallory,
        // authenticating honestly as herself with a wire `group_id` that PASSES
        // `verify_welcome_slot` (`session:bob:mallory`), sends a Welcome whose
        // *embedded* GroupContext id squats `session:alice:bob`. Without the
        // embedded-id binding, `into_group` overwrites Bob's Alice session so his
        // next `encrypt_for_user("alice")` encrypts to Mallory's group. The
        // binding must reject it and leave Bob's Alice session intact.
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
            .unwrap();
        let welcome = alice.create_session("bob").unwrap();
        bob.join_session(&welcome).unwrap();
        assert!(bob.has_session("alice").unwrap());

        // Mallory crafts a group whose embedded id squats bob's alice slot.
        let mallory = create_test_manager("mallory");
        let bob_kp2 = bob.generate_key_package().unwrap();
        let squat = GroupId::new("session:alice:bob").unwrap();
        let cred = mallory.get_credential().unwrap();
        let signer = mallory.get_signer().unwrap();
        mallory
            .group_manager
            .create_group(&squat, &cred, &signer)
            .unwrap();
        let (mal_welcome, _commit) = mallory
            .add_group_member(
                &squat,
                "bob",
                &bob_kp2.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .unwrap();

        // Present it under a wire slot that passes verify_welcome_slot for mallory.
        let attack = WelcomeMessage {
            group_id: GroupId::new("session:bob:mallory").unwrap(),
            welcome_data: mal_welcome.welcome_data,
            inviter_id: "mallory".to_string(),
            group_name: None,
            timestamp_ms: 0,
        };

        let err = bob.join_session(&attack).unwrap_err();
        assert!(
            matches!(err, MlsError::WelcomeGroupIdMismatch { .. }),
            "expected WelcomeGroupIdMismatch, got {:?}",
            err
        );

        // Bob's Alice session survived and still encrypts to Alice's group.
        assert!(bob.has_session("alice").unwrap());
        let ct = bob.encrypt_for_user("alice", b"still private").unwrap();
        let pt = alice.decrypt_from_user(&ct, "bob").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"still private"[..]));
    }

    /// Builds a two-member group (alice admin, bob member) for group
    /// sender-binding tests. Returns (alice, bob, group_id).
    fn create_test_group_with_bob() -> (MlsManager, MlsManager, GroupId) {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
            .unwrap();
        let info = alice.create_group("Test Group").unwrap();
        let gid = info.group_id.clone();
        let (welcome, _commit) = alice
            .add_group_member(
                &gid,
                "bob",
                &bob_kp.key_package_data,
                KeyPackageTrust::FirstUse,
            )
            .unwrap();
        bob.join_group(&welcome).unwrap();
        (alice, bob, gid)
    }

    #[test]
    fn test_group_decrypt_rejects_spoofed_sender() {
        let (alice, bob, gid) = create_test_group_with_bob();

        let ct = alice.encrypt_for_group(&gid, b"group message").unwrap();
        let err = bob.decrypt_from_group(&ct, "mallory").unwrap_err();
        assert!(matches!(err, MlsError::SenderIdentityMismatch { .. }));

        let ct2 = alice.encrypt_for_group(&gid, b"another one").unwrap();
        let pt = bob.decrypt_from_group(&ct2, "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"another one"[..]));
    }

    #[test]
    fn test_group_commit_with_spoofed_sender_rejected_before_merge() {
        let (alice, bob, gid) = create_test_group_with_bob();

        // Alice issues a key-update commit; bob receives it with a spoofed
        // wire sender. The mismatch must be detected BEFORE the staged
        // commit is merged — bob's epoch must not advance.
        let commit = alice.update_keys(&gid).unwrap();
        let epoch_before = bob.get_group_info(&gid).unwrap().unwrap().epoch;

        let err = bob.decrypt_from_group(&commit, "mallory").unwrap_err();
        assert!(matches!(err, MlsError::SenderIdentityMismatch { .. }));

        let epoch_after = bob.get_group_info(&gid).unwrap().unwrap().epoch;
        assert_eq!(
            epoch_before, epoch_after,
            "spoofed commit must not advance group state"
        );
    }

    #[test]
    fn test_no_session_initially() {
        let manager = create_test_manager("alice");
        assert!(!manager.has_session("bob").unwrap());
        assert!(manager.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn test_no_groups_initially() {
        let manager = create_test_manager("alice");
        assert!(manager.list_groups().unwrap().is_empty());
    }

    #[test]
    fn test_group_creation() {
        let manager = create_test_manager("alice");
        let info = manager.create_group("Test Group").unwrap();

        assert_eq!(info.name, Some("Test Group".to_string()));
        assert!(!info.is_session);
        assert_eq!(info.members.len(), 1);

        let groups = manager.list_groups().unwrap();
        assert_eq!(groups.len(), 1);
    }

    /// Regression test for the both-create split-brain re-brick.
    ///
    /// With auto key exchange, both peers create a `session:a:b` group and the
    /// higher-id peer adopts the lower-id "owner"'s Welcome. The owner keeps
    /// retransmitting its Welcome until it sees a group-aware proof, so the
    /// adopter receives the SAME Welcome again *after* it already adopted. MLS
    /// key packages are one-time, so re-staging that Welcome must fail — but it
    /// must fail NON-DESTRUCTIVELY, leaving the converged group intact. The old
    /// delete-then-stage path deleted the good group first and then could not
    /// re-stage, permanently bricking a working session.
    #[test]
    fn test_both_create_adopt_is_non_destructive_on_welcome_retransmit() {
        // alice = lexicographically-lower "owner"; bob = higher "adopter".
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");

        // Exchange key packages both ways (both sides will auto-create).
        let alice_kp = alice.generate_key_package().unwrap();
        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
            .unwrap();
        bob.import_key_package(
            "alice",
            &alice_kp.key_package_data,
            KeyPackageTrust::FirstUse,
        )
        .unwrap();

        // Both-create race: each peer creates its own session group + Welcome.
        let alice_welcome = alice.create_session("bob").unwrap(); // owner's Welcome
        let _bob_welcome = bob.create_session("alice").unwrap(); // adopter's own group
        assert!(bob.has_session("alice").unwrap());

        // Adopter adopts the owner's Welcome (first delivery): replaces its own
        // group with alice's, consuming bob's one-time key package.
        bob.join_session(&alice_welcome).unwrap();
        assert!(bob.has_session("alice").unwrap());

        // Convergence: alice encrypts and bob decrypts on the shared group.
        let ct = alice
            .encrypt_for_user("bob", b"hello over the converged group")
            .unwrap();
        let pt = bob.decrypt_from_user(&ct, "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"hello over the converged group"[..]));

        // Owner retransmits the SAME Welcome (its periodic retry). Re-staging
        // MUST fail (bob's key package is consumed) but MUST be non-destructive.
        let retransmit = bob.join_session(&alice_welcome);
        assert!(
            retransmit.is_err(),
            "re-staging a consumed key package should fail"
        );

        // The converged group MUST survive the failed retransmit. This is the
        // regression: the old delete-then-stage path left bob with no session.
        assert!(
            bob.has_session("alice").unwrap(),
            "duplicate Welcome must not brick the converged session"
        );

        // ...and it must still be functional after the failed retransmit.
        let ct2 = alice
            .encrypt_for_user("bob", b"still converged after retransmit")
            .unwrap();
        let pt2 = bob.decrypt_from_user(&ct2, "alice").unwrap();
        assert_eq!(
            pt2.as_deref(),
            Some(&b"still converged after retransmit"[..])
        );
    }

    /// Windowed media transfers keep up to 8 encrypted chunks in flight
    /// (interleaved with text on the same session ratchet), so a delayed chunk
    /// can arrive many generations behind the newest decrypted message. The
    /// OpenMLS default tolerance (5) would delete its key and permanently stall
    /// the transfer; `SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE` (32) must cover it.
    #[test]
    fn test_out_of_order_decryption_within_sender_ratchet_tolerance() {
        let alice = create_test_manager("alice");
        let bob = create_test_manager("bob");

        let bob_kp = bob.generate_key_package().unwrap();
        alice
            .import_key_package("bob", &bob_kp.key_package_data, KeyPackageTrust::FirstUse)
            .unwrap();
        let welcome = alice.create_session("bob").unwrap();
        bob.join_session(&welcome).unwrap();

        let ciphertexts: Vec<_> = (0..40)
            .map(|i| {
                alice
                    .encrypt_for_user("bob", format!("chunk {}", i).as_bytes())
                    .unwrap()
            })
            .collect();

        // Decrypt the newest message first, ratcheting bob's receive state far
        // ahead of every earlier generation.
        let pt = bob.decrypt_from_user(&ciphertexts[39], "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"chunk 39"[..]));

        // 29 generations behind: far beyond the OpenMLS default of 5, but
        // within our tolerance of 32 — must still decrypt.
        let pt = bob.decrypt_from_user(&ciphertexts[10], "alice").unwrap();
        assert_eq!(pt.as_deref(), Some(&b"chunk 10"[..]));

        // 39 generations behind: beyond the tolerance, the key is deleted and
        // the message must NOT decrypt (proves the configured bound applies).
        let res = bob.decrypt_from_user(&ciphertexts[0], "alice");
        assert!(
            !matches!(res, Ok(Some(_))),
            "generation beyond the tolerance must not decrypt"
        );
    }
}
