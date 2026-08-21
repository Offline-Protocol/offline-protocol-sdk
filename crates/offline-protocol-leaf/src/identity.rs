//! The device's long-term identity, and the MLS client built on it.
//!
//! A leaf node holds one Ed25519 keypair, generated once at provisioning. It
//! signs control frames, it derives the device's address, and it is the
//! signature key inside the device's MLS credential, which is the same three
//! jobs one key does on a phone.
//!
//! # One device, one key
//!
//! A fleet provisioned with a shared identity key is one identity on many
//! devices. Extracting it from a single unit in a laboratory then yields every
//! unit's identity, and the address that names one lock names all of them.
//! This crate cannot prevent a manufacturer from doing that, so it is written
//! down here and in the
//! [threat model](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/security/threat-model.md)
//! as R12.
//!
//! # Where the randomness comes from
//!
//! [`LeafDevice::provision`](crate::LeafDevice::provision) draws from the
//! `getrandom` backend the firmware registers, and the key is exactly as
//! strong as what that returns. This crate deliberately registers no backend
//! of its own: doing so would let a device link and run with entropy this
//! crate invented, which is the one failure that leaves no trace anywhere.

use alloc::{format, string::ToString, sync::Arc, vec, vec::Vec};
use mls_rs::client_builder::MlsConfig;
use mls_rs::identity::basic::{BasicCredential, BasicIdentityProvider};
use mls_rs::identity::SigningIdentity;
use mls_rs::{CipherSuite, CipherSuiteProvider, Client, CryptoProvider};
use mls_rs_core::crypto::{SignaturePublicKey, SignatureSecretKey};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;
use offline_protocol_core::Address;
use offline_protocol_sealed::{derive_address, LEAF_KEY_PACKAGE_LIFETIME};

use crate::adapters::{GroupStateAdapter, KeyPackageAdapter};
use crate::error::{LeafError, Result};
use crate::store::{LeafStore, KEY_TYPE_IDENTITY};

/// The one ciphersuite this protocol uses, and never negotiates.
///
/// `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`, ciphersuite 3. The phone
/// pins the same suite in one place under its own MLS implementation's
/// spelling, and the two names have to mean the same number.
pub const CIPHERSUITE: CipherSuite = CipherSuite::CURVE25519_AES128;

const KEY_ID_SECRET: &str = "signature_secret";
const KEY_ID_PUBLIC: &str = "signature_public";

/// A provisioned device's signing identity.
#[derive(Clone)]
pub(crate) struct Identity {
    pub(crate) secret: SignatureSecretKey,
    pub(crate) public: SignaturePublicKey,
    pub(crate) address: Address,
}

impl Identity {
    /// Generates a fresh identity and writes it before returning it.
    ///
    /// The write comes first for the same reason every other write in this
    /// crate does: a device that hands out an address it did not persist comes
    /// back after a power cut as a different device, and the peer that paired
    /// with the first one has no way to learn that.
    ///
    /// # Why the secret is written last
    ///
    /// An identity is two entries and the store is atomic per entry, not
    /// across a pair, so a cut lands between them. The secret is therefore
    /// both the last write and the marker this function refuses on, which
    /// makes a half-written identity a state the next boot **overwrites**
    /// rather than one it is stuck in.
    ///
    /// Written the other way round the two checks disagree: `resume` refuses
    /// for the missing public key, `provision` refuses for the present secret,
    /// and `open` has no third door. A device that lost power once during its
    /// very first boot would then answer every call with an error, in the
    /// field, with nothing short of an out-of-band wipe to recover it.
    pub(crate) fn provision(store: &Arc<dyn LeafStore>) -> Result<Self> {
        if store
            .load(KEY_TYPE_IDENTITY, KEY_ID_SECRET)
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .is_some()
        {
            return Err(LeafError::AlreadyProvisioned);
        }

        let suite = suite_provider()?;
        let (secret, public) = suite
            .signature_key_generate()
            .map_err(|e| LeafError::Crypto(format!("cannot generate a signature key: {e:?}")))?;

        store
            .store(KEY_TYPE_IDENTITY, KEY_ID_PUBLIC, public.as_bytes())
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        store
            .store(KEY_TYPE_IDENTITY, KEY_ID_SECRET, secret.as_bytes())
            .map_err(|e| LeafError::Storage(e.to_string()))?;

        let address = derive_address(public.as_bytes())?;
        Ok(Self {
            secret,
            public,
            address,
        })
    }

    /// Loads a previously provisioned identity.
    ///
    /// Both entries are required, which is safe only because
    /// [`Identity::provision`] writes the public key first: the pair is either
    /// complete or missing the secret, and the second of those is what `open`
    /// recovers from. Swapping those two writes makes this function the half
    /// of a deadlock.
    pub(crate) fn resume(store: &Arc<dyn LeafStore>) -> Result<Self> {
        let secret = store
            .load(KEY_TYPE_IDENTITY, KEY_ID_SECRET)
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .ok_or(LeafError::NotProvisioned)?;
        let public = store
            .load(KEY_TYPE_IDENTITY, KEY_ID_PUBLIC)
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .ok_or(LeafError::NotProvisioned)?;

        let public = SignaturePublicKey::from(public);
        let address = derive_address(public.as_bytes())?;
        Ok(Self {
            secret: SignatureSecretKey::from(secret),
            public,
            address,
        })
    }

    /// The credential this device presents inside MLS.
    ///
    /// A basic credential whose whole content is the device's address in
    /// canonical text. RFC 9420 calls a basic credential "a bare assertion of
    /// an identity", which is exactly what it is: the assertion means
    /// something only because the address is the hash of the signature key
    /// beside it, so a verifier re-derives rather than trusts.
    pub(crate) fn signing_identity(&self) -> SigningIdentity {
        let credential = BasicCredential::new(self.address.to_string().into_bytes());
        SigningIdentity::new(credential.into_credential(), self.public.clone())
    }
}

/// The crypto provider, restricted to the one suite.
///
/// `with_enabled_cipher_suites` is a runtime filter rather than a compile-time
/// one: the provider keeps all four curves in one enum, so P-384 and P-256
/// arithmetic link into the image whatever this says. Restricting it is still
/// right, because it makes a request for another suite fail here rather than
/// succeed with a suite the peer does not speak.
pub(crate) fn crypto_provider() -> RustCryptoProvider {
    RustCryptoProvider::with_enabled_cipher_suites(vec![CIPHERSUITE])
}

/// The suite provider, for the raw sign and verify a control frame needs.
pub(crate) fn suite_provider() -> Result<<RustCryptoProvider as CryptoProvider>::CipherSuiteProvider>
{
    crypto_provider()
        .cipher_suite_provider(CIPHERSUITE)
        .ok_or_else(|| {
            LeafError::Crypto(alloc::string::String::from(
                "the crypto provider does not support the protocol's ciphersuite",
            ))
        })
}

/// Builds the MLS client for this device.
///
/// Storage is the device's own, through the adapters, so the client reads and
/// writes the same blobs across a power cycle. Nothing about the group lives
/// in this value: it is rebuilt per operation and the group is loaded from
/// storage, which is what makes storage the source of truth rather than a
/// cache of something held in RAM.
pub(crate) fn build_client(
    identity: &Identity,
    store: &Arc<dyn LeafStore>,
) -> Result<Client<impl MlsConfig>> {
    Ok(Client::builder()
        .identity_provider(BasicIdentityProvider)
        .crypto_provider(crypto_provider())
        .group_state_storage(GroupStateAdapter::new(Arc::clone(store)))
        .key_package_repo(KeyPackageAdapter::new(Arc::clone(store)))
        .signing_identity(
            identity.signing_identity(),
            identity.secret.clone(),
            CIPHERSUITE,
        )
        .key_package_lifetime(LEAF_KEY_PACKAGE_LIFETIME)
        .build())
}

/// Signs `payload` with the device's identity key.
pub(crate) fn sign(identity: &Identity, payload: &[u8]) -> Result<Vec<u8>> {
    suite_provider()?
        .sign(&identity.secret, payload)
        .map_err(|e| LeafError::Crypto(format!("cannot sign: {e:?}")))
}

/// Verifies `signature` over `payload` under `public_key`.
pub(crate) fn verify(public_key: &[u8], signature: &[u8], payload: &[u8]) -> Result<()> {
    let key = SignaturePublicKey::from(public_key.to_vec());
    suite_provider()?
        .verify(&key, signature, payload)
        .map_err(|e| LeafError::ControlFrameRefused(format!("signature does not verify: {e:?}")))
}
