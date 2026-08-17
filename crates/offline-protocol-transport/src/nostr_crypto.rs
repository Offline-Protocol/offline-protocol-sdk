//! Nostr event creation and BIP-340 Schnorr signing.
//!
//! Implements NIP-01 event serialization, SHA-256 event ID computation,
//! and BIP-340 Schnorr signatures using the secp256k1 curve (via `k256`).
//!
//! The platform side (iOS/Android) no longer handles crypto — it receives
//! pre-signed event JSON strings ready for relay submission.
//!
//! # Key model
//!
//! Three values are in play, and they must not be conflated:
//!
//! - **Routing tag** ([`routing_tag_for_address`]): a public rendezvous label
//!   derived deterministically from a device's address. Senders put the
//!   recipient's tag in the event's `#p` tag; the recipient subscribes on its
//!   own. It is a *label*: nothing signs with it, nothing seals to it, and
//!   anyone who knows an address can — and must be able to — compute it,
//!   exactly like an email address.
//! - **Signing key** ([`NostrKeypair::from_install_secret`]): the secp256k1
//!   private key that signs outgoing events, derived via HKDF-SHA256 from a
//!   per-install random secret. Never derivable from any public value.
//! - **Record-seal key** ([`record_seal_keypair_for_address`]): a keypair
//!   *anyone who knows an address can reconstruct*, holding one job — sealing
//!   the published key-package record and the bootstrap leg of a conversation,
//!   so a relay scraping by kind sees ciphertext instead of a directory. Its
//!   private half is public by construction, so it must never authenticate
//!   anything.
//!
//! Two historical conflations are worth knowing about, because both looked
//! harmless until they weren't.
//!
//! Originally the *signing* key was `SHA-256(device_id)` too, so anyone who
//! knew a device id could sign events as that device. That one was fixed by
//! moving signing onto a per-install secret.
//!
//! What survived until the address migration was subtler: the record-seal
//! key's public half **was** the routing tag, bit for bit. Nothing was wrong
//! with it on its own — the two roles happened to want the same derivation —
//! but it left every routing tag standing as a live encryption key, which is a
//! footgun the moment anything adds NIP-42 AUTH or pubkey-based filtering. The
//! seal key now comes from its own domain-separated HKDF, so the two are
//! deliberately unequal and nothing seals to a tag any more — pinned by
//! `test_record_seal_key_is_not_the_routing_tag`.
//!
//! **What did not change: a routing tag's private half is still computable.**
//! Its scalar *is* `SHA-256(address)` (the tag itself is that scalar's x-only
//! public key), so anyone holding an address can reconstruct the keypair behind
//! that address's tag — and must be able to, since that is how senders address
//! it. What the split bought is that reconstructing one now decrypts nothing.
//! See [`routing_tag_for_address`].

use crate::constants::{NOSTR_CREATED_AT_JITTER_SECS, NOSTR_INITIAL_QUERY_LIMIT};
use crate::nip44::{self, ConversationKey};
use crate::{Error, Result};
use hkdf::Hkdf;
use k256::schnorr::SigningKey;
use offline_protocol_core::{Address, Username};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Domain-separation prefix for deriving the Nostr signing key from the
/// per-install secret. A trailing counter byte is appended per attempt so an
/// (astronomically unlikely) invalid scalar can be retried deterministically.
const SIGNING_KEY_HKDF_INFO: &[u8] = b"offline-protocol/nostr/v1/signing-key/";

/// Domain-separation prefix for the publicly computable record-seal key.
///
/// Distinct from [`SIGNING_KEY_HKDF_INFO`] because these two keys have
/// opposite security properties — one is an unforgeable identity, the other is
/// reconstructible by anyone holding the address — and sharing a derivation
/// between them is the kind of thing that is fine until someone reuses one
/// where they meant the other.
const RECORD_SEAL_HKDF_INFO: &[u8] = b"offline-protocol/nostr/v1/record-seal-key/";

/// Domain-separation prefix for the publicly computable discovery-seal key.
///
/// Structurally identical to [`RECORD_SEAL_HKDF_INFO`], different info string,
/// and keyed on a *username* rather than an address — which is the property
/// that makes it a weaker key and the reason the invariant below is stated
/// rather than assumed. See [`discovery_seal_keypair_for_username`].
const DISCOVERY_SEAL_HKDF_INFO: &[u8] = b"offline-protocol/nostr/v1/discovery-seal-key/";

/// Domain separator for the discovery tag's hash preimage.
///
/// The trailing colon is part of the constant: without a separator,
/// `"offline-disc-v1" ‖ username` and a username that happened to begin with
/// the tail of the domain would share a preimage.
const DISCOVERY_TAG_DOMAIN: &[u8] = b"offline-disc-v1:";

/// Upper bound on HKDF derivation attempts. Each attempt fails with
/// probability ~2^-128 (scalar of zero or above the curve order), so more
/// than one iteration is never expected in practice.
const MAX_DERIVE_ATTEMPTS: u8 = 8;

/// Minimum accepted install-secret length in bytes (128-bit security floor).
const MIN_INSTALL_SECRET_LEN: usize = 16;

/// A Nostr signing keypair for this install.
///
/// The private key is derived from a per-install random secret via
/// HKDF-SHA256 with domain separation (see [`Self::from_install_secret`]),
/// or freshly generated ([`Self::generate_ephemeral`]) when no persisted
/// secret is available yet. The public key is the x-only coordinate
/// (32 bytes, 64 hex chars) per BIP-340.
///
/// The keypair authenticates this install to Nostr relays (event signatures,
/// NIP-42 style auth). It is intentionally *not* derivable from the device
/// ID; message addressing uses the separate [`routing_tag_for_address`]
/// label instead.
pub struct NostrKeypair {
    signing_key: SigningKey,
    public_key_hex: String,
}

impl NostrKeypair {
    /// Derives the signing keypair from a per-install random secret.
    ///
    /// Derivation: `HKDF-SHA256(ikm = secret, salt = none,
    /// info = "offline-protocol/nostr/v1/signing-key/" || counter)` → 32-byte
    /// scalar → secp256k1 signing key. Deterministic: the same secret always
    /// yields the same keypair, so the install's Nostr identity is stable
    /// across restarts as long as the secret persists.
    ///
    /// Rejects secrets shorter than 16 bytes.
    pub fn from_install_secret(secret: &[u8]) -> Result<Self> {
        if secret.len() < MIN_INSTALL_SECRET_LEN {
            return Err(Error::CryptoError(format!(
                "Install secret too short: {} bytes (minimum {})",
                secret.len(),
                MIN_INSTALL_SECRET_LEN
            )));
        }

        let hkdf = Hkdf::<Sha256>::new(None, secret);
        let mut info = Vec::with_capacity(SIGNING_KEY_HKDF_INFO.len() + 1);
        for counter in 0..MAX_DERIVE_ATTEMPTS {
            info.clear();
            info.extend_from_slice(SIGNING_KEY_HKDF_INFO);
            info.push(counter);

            let mut candidate = Zeroizing::new([0u8; 32]);
            hkdf.expand(&info, &mut *candidate)
                .map_err(|e| Error::CryptoError(format!("HKDF expand failed: {}", e)))?;

            if let Ok(signing_key) = SigningKey::from_bytes(&*candidate) {
                let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
                return Ok(Self {
                    signing_key,
                    public_key_hex,
                });
            }
        }

        Err(Error::CryptoError(
            "Failed to derive a valid signing key from the install secret".to_string(),
        ))
    }

    /// Generates a keypair from a fresh random secret that is not retained.
    ///
    /// Used as the construction-time default before a persisted install
    /// secret is available: the key is unforgeable but changes on every
    /// process start. Callers that want a stable identity install a
    /// persisted secret afterwards via [`Self::from_install_secret`].
    pub fn generate_ephemeral() -> Result<Self> {
        let secret = Self::generate_install_secret()?;
        Self::from_install_secret(&*secret)
    }

    /// Generates a new 32-byte install secret from the OS CSPRNG.
    ///
    /// The caller is responsible for persisting it in platform-secure
    /// storage; this module never stores anything. The returned buffer is
    /// zeroed on drop.
    pub fn generate_install_secret() -> Result<Zeroizing<[u8; 32]>> {
        let mut secret = Zeroizing::new([0u8; 32]);
        OsRng
            .try_fill_bytes(&mut *secret)
            .map_err(|e| Error::CryptoError(format!("OS RNG failure: {}", e)))?;
        Ok(secret)
    }

    /// Returns the x-only public key as a 64-character lowercase hex string.
    pub fn public_key_hex(&self) -> &str {
        &self.public_key_hex
    }
}

/// Reconstructs the keypair that seals content addressed to `address`.
///
/// # This key is not a secret
///
/// It is derived from the address alone, so **anyone who knows an address
/// holds this keypair for it**. That is the point: it lets a stranger seal a
/// first frame to a peer they have never exchanged keys with, and lets them
/// open that peer's published key-package record — while a relay scraping by
/// kind, which knows tags but not addresses, sees only ciphertext.
///
/// What that is worth changed with the address migration, and it is worth
/// being precise about. When the preimage was a username, "anyone who knows
/// it" meant anyone who could *guess* `bob`. An address is a 160-bit hash of
/// an identity key: it cannot be guessed, only learned — from an invite, a QR
/// code, or a frame the holder already sent you. So the audience for this key
/// is now the same set of people who could already send you traffic, rather
/// than everyone with a dictionary.
///
/// It must **never** authenticate anything: not NIP-42 AUTH, not inbound
/// sender attribution, not any decision that treats "this decrypted" as
/// evidence of who sent it. Sender authenticity comes from the protocol-layer
/// Ed25519 + derivation gate and from MLS, both above this function.
/// [`NostrKeypair::from_install_secret`] is the only unforgeable identity
/// here.
///
/// Domain-separated so this is *not* the routing tag's scalar. Nothing breaks
/// if they coincide, but a routing label that doubles as a public key whose
/// private half is computable is a trap for whoever next touches this file.
///
/// Takes an [`Address`] rather than a string for the reason given on
/// [`routing_tag_for_address`]: the whole point of this key is that its
/// preimage cannot be guessed, which is a property of addresses and not of
/// strings.
pub fn record_seal_keypair_for_address(address: &Address) -> Result<NostrKeypair> {
    let address = address.to_string();
    let hkdf = Hkdf::<Sha256>::new(None, address.as_bytes());
    let mut info = Vec::with_capacity(RECORD_SEAL_HKDF_INFO.len() + 1);
    for counter in 0..MAX_DERIVE_ATTEMPTS {
        info.clear();
        info.extend_from_slice(RECORD_SEAL_HKDF_INFO);
        info.push(counter);

        let mut candidate = Zeroizing::new([0u8; 32]);
        hkdf.expand(&info, &mut *candidate)
            .map_err(|e| Error::CryptoError(format!("HKDF expand failed: {}", e)))?;

        if let Ok(signing_key) = SigningKey::from_bytes(&*candidate) {
            let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
            return Ok(NostrKeypair {
                signing_key,
                public_key_hex,
            });
        }
    }

    Err(Error::CryptoError(
        "Failed to derive a record-seal key for the address".to_string(),
    ))
}

/// Current wall-clock time as unix seconds — the unit every Nostr timestamp
/// (`created_at`, filter `since`/`until`) is expressed in.
///
/// Saturates to 0 rather than panicking if the system clock is set before the
/// epoch; a zero here only widens a subscription window.
pub(crate) fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Computes the public routing tag for a device's address.
///
/// Derivation: `SHA-256(address)` → scalar → x-only secp256k1 public key hex.
///
/// Senders place the recipient's tag in the `#p` tag of outgoing events and
/// recipients subscribe on their own. It is a rendezvous label only: nothing
/// signs with the corresponding scalar, nothing seals to it, and incoming
/// events are never authenticated against it (sender authenticity comes from
/// the protocol layer).
///
/// **The scalar is not a secret and is not claimed to be.** It *is*
/// `SHA-256(address)`, so anyone holding an address can reconstruct the whole
/// keypair behind that address's tag. That is harmless only because of the
/// sentence above — nothing signs with it and nothing seals to it — and it is
/// what makes the previous shape a trap worth having removed: the key that
/// does seal is now a separate domain-separated derivation
/// ([`record_seal_keypair_for_address`]), so reconstructing a tag decrypts
/// nothing.
///
/// # Why an identity-derived label is acceptable here
///
/// Deriving a delivery address from stable identity material is normally the
/// wrong move — it lets an observer precompute the label for any identity it
/// cares about and confirm traffic without holding a key. The Marmot protocol
/// forbids it for *group* addressing for exactly that reason, and carves out
/// the case this is: an account-level inbox, "because reaching a specific
/// account is the purpose of an inbox". NIP-59 goes further and puts the
/// recipient's actual pubkey in `p`.
///
/// So the tag is not obscurity and is not load-bearing for privacy. What it
/// does buy is non-invertibility: knowing a tag does not yield the address, so
/// a relay cannot turn its subscriber list back into an address book. That
/// property is the reason the published record stays sealed — publishing it in
/// the clear would hand the address back at the tag it sits on.
///
/// # Why the parameter is an [`Address`] and not a string
///
/// Every sentence above depends on the preimage being an address. Feed this a
/// username and it computes a perfectly valid tag over a guessable string,
/// which is precisely the leak the addressing migration removed: anyone with a
/// dictionary recovers the label and watches that inbox. That made the
/// invariant real but unrepresentable — enforced by three separate `parse`
/// gates at the call sites, each of which had to be remembered.
///
/// Taking the parsed type instead moves the check to the boundary where the
/// value enters the transport, and makes "derive a tag for `bob`" fail to
/// compile rather than fail in review. The gates stay: they are now the parse
/// points that *produce* this type.
pub fn routing_tag_for_address(address: &Address) -> Result<String> {
    let address = address.to_string();
    let tag_scalar = Sha256::digest(address.as_bytes());
    let tag_key = SigningKey::from_bytes(tag_scalar.as_slice())
        .map_err(|e| Error::CryptoError(format!("Invalid routing tag for address: {}", e)))?;
    Ok(hex::encode(tag_key.verifying_key().to_bytes()))
}

/// Computes the public discovery tag for a username.
///
/// Derivation: `SHA-256("offline-disc-v1:" ‖ username)` → scalar → x-only
/// secp256k1 public key hex. The scalar→pubkey step mirrors
/// [`routing_tag_for_address`] so the published value is shaped like any other
/// `#p` pubkey and one relay-side code path serves both.
///
/// # Why the preimage is domain-separated
///
/// The naive framing overstates the direct damage, so be precise about what
/// this buys. If the preimage were the bare username, a user who registered
/// the literal string `off1qys…` would derive *that address's* key-package tag.
/// The immediate harm is small — the two record kinds are separately queryable
/// so results do not cross-contaminate, and the seal keys already differ by
/// their HKDF info string.
///
/// What the separator actually buys is that the two namespaces stop sharing a
/// preimage space at all, so no *future* third derivation can be aimed across
/// them, and "refuse address-shaped usernames"
/// ([`UsernameError::AddressShaped`](offline_protocol_core::UsernameError))
/// becomes belt-and-braces rather than the only thing standing between the two.
/// One constant, one class of bug removed.
///
/// # Why the parameter is a [`Username`] and not a string
///
/// The tag is a hash of the *normalized* name. Two callers that normalize
/// differently derive different tags and silently fail to find each other —
/// there is no error to observe, only an empty result for a name that exists.
/// Taking the parsed type moves normalization to the boundary and makes
/// "derive a tag for whatever the app typed" fail to compile, the same move
/// [`routing_tag_for_address`] makes for addresses.
pub fn discovery_tag_for_username(username: &Username) -> Result<String> {
    let mut preimage = Vec::with_capacity(DISCOVERY_TAG_DOMAIN.len() + username.as_str().len());
    preimage.extend_from_slice(DISCOVERY_TAG_DOMAIN);
    preimage.extend_from_slice(username.as_str().as_bytes());

    let tag_scalar = Sha256::digest(&preimage);
    let tag_key = SigningKey::from_bytes(tag_scalar.as_slice())
        .map_err(|e| Error::CryptoError(format!("Invalid discovery tag for username: {}", e)))?;
    Ok(hex::encode(tag_key.verifying_key().to_bytes()))
}

/// Reconstructs the keypair that seals a username's discovery records.
///
/// Derivation: `HKDF-SHA256(ikm = normalized username, salt = none,
/// info = "offline-protocol/nostr/v1/discovery-seal-key/" ‖ counter)`.
///
/// # This key is public by construction, and that is deliberate
///
/// **It reintroduces the exact class the address migration deleted**: a
/// keypair derived from a *guessable* string. That is not an oversight and it
/// must not be quietly "fixed" by a later change that does not know why it is
/// here, so the reasoning is recorded at the definition site.
///
/// The invariant that makes it acceptable is the same one in both directions:
/// **this key must never be load-bearing.** It is an opaque discovery label. It
/// must never back encryption of anything secret, never back NIP-42 AUTH, and
/// never inform any authentication decision. Every authenticity property of a
/// discovery record comes from the Ed25519 signature inside it and from
/// `derive_address(pubkey) == address` — nothing rests on who could open the
/// seal.
///
/// What sealing buys, given that anyone who knows the username can unseal, is
/// resistance to *bulk collection*. Publishing the records in the clear would
/// let a single `{"kinds":[30777]}` request return a directory of every
/// username on the relay paired with its address — strictly worse than the leak
/// the record-seal key was introduced to close, since there the pairing had to
/// be inferred and here the mapping *is* the payload. Sealing costs nothing in
/// reach: fetching requires the tag, the tag requires the username, and the
/// username is what reconstructs this key.
///
/// Deliberately not numerically the discovery tag, for the same reason
/// [`record_seal_keypair_for_address`] is not the routing tag; pinned by
/// `test_discovery_seal_key_is_not_the_discovery_tag`.
pub fn discovery_seal_keypair_for_username(username: &Username) -> Result<NostrKeypair> {
    let hkdf = Hkdf::<Sha256>::new(None, username.as_str().as_bytes());
    let mut info = Vec::with_capacity(DISCOVERY_SEAL_HKDF_INFO.len() + 1);
    for counter in 0..MAX_DERIVE_ATTEMPTS {
        info.clear();
        info.extend_from_slice(DISCOVERY_SEAL_HKDF_INFO);
        info.push(counter);

        let mut candidate = Zeroizing::new([0u8; 32]);
        hkdf.expand(&info, &mut *candidate)
            .map_err(|e| Error::CryptoError(format!("HKDF expand failed: {}", e)))?;

        if let Ok(signing_key) = SigningKey::from_bytes(&*candidate) {
            let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
            return Ok(NostrKeypair {
                signing_key,
                public_key_hex,
            });
        }
    }

    Err(Error::CryptoError(
        "Failed to derive a discovery-seal key for the username".to_string(),
    ))
}

/// Event kind for a NIP-59 gift wrap. Taken in isolation one of our events is
/// an ordinary wrapped DM — kind, unlinkable per-event pubkey, opaque `#p`
/// tag, coarse timestamp, ciphertext — which is why we reuse the wrapper
/// instead of minting a kind of our own: a scrape *by kind alone* cannot
/// enumerate the userbase.
///
/// # What this does not buy
///
/// The anonymity set is **not** "every NIP-17 conversation on the relay", and
/// it is worth being exact about that, because a relay separates our tags from
/// generic NIP-17 recipients in several ways and none of them has to break the
/// sealing to do it:
///
/// - **Our own subscription names the tag.** `NostrTransport::create_subscription`
///   sends `{"#p":[<our tag>], "kinds":[4, 1059], …}` to every relay we connect
///   to, so a relay serving that `REQ` learns the tag belongs to the connected
///   client directly, with no correlation work at all. This one is decisive and
///   unavoidable while the recipient subscribes on a stable tag.
/// - **The `kinds` pair is a fingerprint.** Requesting the legacy kind 4
///   alongside 1059 under a single `#p` is a shape a NIP-17-only client does
///   not have.
/// - **The key-package record cross-links the tag.** With cold contact enabled
///   (the default), [`NOSTR_KEY_PACKAGE_KIND`] records sit at our *own* routing
///   tag and are signed by the install's real Nostr key, so a relay can join
///   the tag to that pubkey and mark it as an MLS-over-Nostr client rather than
///   a generic correspondent.
///
/// What the wrapper does buy is that none of that reveals *content*: who is
/// talking to whom, about what, and under which app id all stay sealed. The
/// gift wrap hides the conversation, not the fact that a given tag is one of
/// ours to a relay we speak to.
pub const NOSTR_GIFT_WRAP_KIND: u32 = 1059;

/// Legacy NIP-04 direct-message kind. Deprecated upstream (`unrecommended`,
/// superseded by NIP-17) and, as used here historically, published the whole
/// protocol envelope in cleartext. Retained only so the receive path keeps
/// parsing frames from peers that predate sealing.
pub const NOSTR_LEGACY_DM_KIND: u32 = 4;

/// Event kind for a published MLS key package: a NIP-33 *addressable* event,
/// so republishing a slot replaces the record in place rather than appending
/// to a growing pile a fetcher would have to sort through.
///
/// 30443 is the kind the Marmot protocol publishes key packages under. Sharing
/// it is deliberate and follows the same reasoning as reusing NIP-59's wrapper
/// for sealed traffic: a kind of our own would let one relay filter enumerate
/// exactly the Offline Protocol userbase, whereas this way the set is every
/// client that publishes MLS key packages to Nostr. The payloads are not
/// interchangeable — a Marmot client fetching ours reads a NIP-44 blob it holds
/// no key for — but interoperability was never the point; the anonymity set is.
///
/// That set is the honest bound for a scrape *by kind*, and no larger. The
/// record is published at our own routing tag and signed by the install's real
/// Nostr key, so a relay holding it can join the two — which is one of the ways
/// a routing tag is distinguishable from a generic NIP-17 recipient's, listed
/// on [`NOSTR_GIFT_WRAP_KIND`].
pub const NOSTR_KEY_PACKAGE_KIND: u32 = 30443;

/// Event kind for a published username discovery record: addressable, so a
/// republished claim *replaces* the previous one.
///
/// # Why a kind of our own, when the key-package record shares Marmot's
///
/// The key-package record shares 30443 to join an anonymity set: several
/// protocols publish MLS key packages to Nostr, so a scrape by kind alone
/// cannot enumerate this SDK's userbase. There is no equivalent set to join for
/// a username directory — no other protocol publishes one — so a distinct kind
/// buys clean filtering and costs nothing that was available to take.
///
/// 30777 is **unregistered**. Nothing in the NIPs kind registry is assigned
/// anywhere in the 30700–30800 range (re-checked 2026-08-17), and nothing
/// prevents use of an unassigned kind. If this format is ever published as a
/// specification, whether to seek a registry entry or accept the collision risk
/// of an unregistered kind is an open question recorded in
/// `docs/spec/username-discovery.md`.
pub const NOSTR_DISCOVERY_KIND: u32 = 30777;

/// NIP-09 deletion-request kind, used for the best-effort half of a retraction.
///
/// Best-effort by nature: a relay may honour it, ignore it, or honour it only
/// for events it still holds. The durable half of a retraction is republishing
/// the addressable slot with a tombstone body, which uses the replacement rule
/// rather than the relay's goodwill.
pub const NOSTR_DELETION_KIND: u32 = 5;

/// Picks a `created_at` uniformly in `[now - NOSTR_CREATED_AT_JITTER_SECS, now]`.
///
/// NIP-59 requires the wrapper's timestamp be randomized into the **past** —
/// never the future — so a relay cannot use publication time to correlate a
/// wrapper with the conversation it belongs to, and so a clock-skewed client
/// does not publish events its peers' `until` filters reject.
///
/// Past-only is also what makes the subscription window sound: `since` is
/// derived from the receive watermark minus this same constant (see
/// `NostrTransport::subscription_since`), so an event jittered to the far edge
/// of the window is still inside the next query. Widening the jitter without
/// widening that overlap silently drops messages.
fn jittered_created_at() -> i64 {
    let now = now_unix_secs();
    let mut buf = [0u8; 8];
    if OsRng.try_fill_bytes(&mut buf).is_err() {
        // Randomness is a privacy input here, not a security one: without it
        // the timestamp is merely un-jittered, which is what every event
        // published before this change already looked like. Failing the send
        // instead would turn an RNG hiccup into a delivery outage.
        return now;
    }
    let offset = (u64::from_be_bytes(buf) % (NOSTR_CREATED_AT_JITTER_SECS as u64 + 1)) as i64;
    now.saturating_sub(offset).max(0)
}

/// A fully signed NIP-01 Nostr event ready for relay submission.
#[derive(Debug, Clone)]
pub struct NostrEvent {
    /// Event ID: 32-byte hex SHA-256 of the canonical serialization.
    pub id: String,
    /// Sender's x-only public key (32-byte hex).
    pub pubkey: String,
    /// Unix timestamp in seconds.
    pub created_at: i64,
    /// Event kind (4 = NIP-04 direct message).
    pub kind: u32,
    /// Event tags (e.g., `[["p", "<recipient_pubkey>"]]`).
    pub tags: Vec<Vec<String>>,
    /// Event content (base64-encoded protocol message).
    pub content: String,
    /// BIP-340 Schnorr signature (64-byte hex).
    pub sig: String,
}

impl NostrEvent {
    /// Creates and signs a NIP-04 direct message event.
    ///
    /// - `keypair`: Sender's signing keypair (per-install secret key).
    /// - `recipient_pubkey_hex`: Recipient's routing tag (64-char hex, from
    ///   [`routing_tag_for_address`]).
    /// - `content_base64`: Base64-encoded protocol message bytes.
    pub fn create_dm(
        keypair: &NostrKeypair,
        recipient_pubkey_hex: &str,
        content_base64: &str,
    ) -> Result<Self> {
        Self::sign(
            keypair,
            NOSTR_LEGACY_DM_KIND,
            recipient_pubkey_hex,
            content_base64,
            now_unix_secs(),
        )
    }

    /// Creates a NIP-59 gift wrap (kind [`NOSTR_GIFT_WRAP_KIND`]) carrying
    /// `plaintext` sealed to `recipient_encryption_pubkey`.
    ///
    /// - `recipient_tag`: the recipient's routing tag, the `#p` value they
    ///   subscribe on. Addressing only — it is not necessarily the key the
    ///   payload is sealed to.
    /// - `recipient_encryption_pubkey`: the x-only key the NIP-44 layer is
    ///   sealed to. Either the recipient's real per-install Nostr key (once we
    ///   have learned it from their key package) or, on the bootstrap leg,
    ///   their publicly computable key — which is the same value as
    ///   `recipient_tag`. The two arguments are kept separate precisely so that
    ///   coincidence stays visible at every call site rather than being assumed.
    ///
    /// The wrapper is signed by a **freshly generated, single-use** keypair, so
    /// no two events this device publishes are linkable to each other or back
    /// to it. That key is discarded when this function returns; nothing
    /// retains it, and nothing needs to — the recipient recovers the shared
    /// secret from the event's own `pubkey` field.
    pub fn create_gift_wrap(
        recipient_tag: &str,
        recipient_encryption_pubkey: &str,
        plaintext: &[u8],
    ) -> Result<Self> {
        let ephemeral = NostrKeypair::generate_ephemeral()?;
        let peer = hex::decode(recipient_encryption_pubkey).map_err(|e| {
            Error::CryptoError(format!("Invalid recipient encryption pubkey: {}", e))
        })?;
        let conversation_key = ConversationKey::derive(&ephemeral.signing_key, &peer)?;
        let sealed = nip44::encrypt(plaintext, &conversation_key)?;

        Self::sign(
            &ephemeral,
            NOSTR_GIFT_WRAP_KIND,
            recipient_tag,
            &sealed,
            jittered_created_at(),
        )
    }

    /// Creates a published key package record: an addressable
    /// [`NOSTR_KEY_PACKAGE_KIND`] event carrying `plaintext` at slot `slot_id`.
    ///
    /// Unlike a gift wrap this is signed by the install's **real** signing key,
    /// and that is the point of the record: a stranger who fetches it learns the
    /// key to seal to from the event's own `pubkey` field, which is how the
    /// bootstrap leg stops being necessary. (That field is only self-attesting —
    /// it is BIP-340-signed by the very key it names and bound to no user id —
    /// so the *engine* still takes the peer's key from the Ed25519-signed
    /// `nostr_pubkey` inside the payload. This function's job is to make the
    /// record fetchable, not to make it trusted.)
    ///
    /// # Why the content is sealed even though the record is public
    ///
    /// The original reason was that an MLS key package carries its owner's
    /// user id twice over — in the `KeyPackagePayload` field and, unremovably,
    /// in the leaf credential — so a cleartext record would have let
    /// `{"kinds":[30443]}` return **a directory of every username on the
    /// relay**. That reason expired: credentials now hold the derived address,
    /// and an address is not a name.
    ///
    /// The seal stays anyway, for a narrower reason that did not expire. The
    /// routing tag is one-way — a relay holding tags cannot recover addresses
    /// from them. A cleartext record publishes the address *at* its own tag,
    /// which hands that inversion back for the whole userbase to anyone willing
    /// to scrape one kind. Sealing keeps the tag one-way.
    ///
    /// Worth knowing that this is where we diverge from Marmot, which
    /// publishes kind-30443 key packages in the clear and is right to: their
    /// leaf credential *is* the Nostr pubkey the event is already signed by, so
    /// a cleartext record discloses nothing the event's own `pubkey` field did
    /// not. Ours names a different identity, so ours has something to hide.
    ///
    /// Sealing costs nothing in reach: opening the record needs the address,
    /// and so does finding it — the tag you fetch from is derived from that
    /// same address.
    ///
    /// `created_at` is the true current time, deliberately **not** jittered into
    /// the past like a gift wrap's. Relays keep the newest event per
    /// `(kind, pubkey, d)` and drop anything older, so a backdated republication
    /// would be silently discarded — leaving a consumed key package standing as
    /// the live record. Nothing is lost by it: this is a standing record, not a
    /// message, so its timestamp correlates with no conversation.
    ///
    /// `routing_tag` addresses the record; `seal_pubkey` is our own
    /// record-seal public key, which is what a fetcher reconstructs from our
    /// address to open it. They used to be the same value and are now
    /// deliberately not, so the two arguments say which job each is doing.
    pub fn create_key_package_publication(
        keypair: &NostrKeypair,
        routing_tag: &str,
        seal_pubkey: &str,
        slot_id: &str,
        plaintext: &[u8],
    ) -> Result<Self> {
        let seal_bytes = hex::decode(seal_pubkey)
            .map_err(|e| Error::CryptoError(format!("Invalid record-seal pubkey: {}", e)))?;
        let conversation_key = ConversationKey::derive(&keypair.signing_key, &seal_bytes)?;
        let sealed = nip44::encrypt(plaintext, &conversation_key)?;

        let tags = vec![
            vec!["d".to_string(), slot_id.to_string()],
            vec!["p".to_string(), routing_tag.to_string()],
        ];
        Self::sign_with_tags(
            keypair,
            NOSTR_KEY_PACKAGE_KIND,
            tags,
            &sealed,
            now_unix_secs(),
        )
    }

    /// Builds a sealed, addressable username discovery record.
    ///
    /// `discovery_tag` addresses the record and is **also its `d` tag**, which
    /// is the one structural difference from
    /// [`Self::create_key_package_publication`] and is deliberate. Key packages
    /// need random slot ids because an init key is single-use and two fetchers
    /// must not collide, so their records accumulate. A directory entry is the
    /// opposite: it is a *statement* that should be replaced rather than
    /// accumulated, and a deterministic `d` is exactly what makes NIP-01
    /// addressable replacement do that work. Setting `d` to the tag duplicates
    /// a value that is already public, so it leaks nothing new, and it leaves a
    /// device a natural way to claim more than one username — one record per
    /// `d`.
    ///
    /// `created_at` is the true current time and is deliberately **not**
    /// jittered, for the same reason the key-package record is not: relays keep
    /// the newest event per `(kind, pubkey, d)`, so a backdated republication
    /// is silently dropped and would strand a stale claim standing as live.
    pub fn create_discovery_publication(
        keypair: &NostrKeypair,
        discovery_tag: &str,
        seal_pubkey: &str,
        plaintext: &[u8],
    ) -> Result<Self> {
        let seal_bytes = hex::decode(seal_pubkey)
            .map_err(|e| Error::CryptoError(format!("Invalid discovery-seal pubkey: {}", e)))?;
        let conversation_key = ConversationKey::derive(&keypair.signing_key, &seal_bytes)?;
        let sealed = nip44::encrypt(plaintext, &conversation_key)?;

        let tags = vec![
            vec!["d".to_string(), discovery_tag.to_string()],
            vec!["p".to_string(), discovery_tag.to_string()],
        ];
        Self::sign_with_tags(
            keypair,
            NOSTR_DISCOVERY_KIND,
            tags,
            &sealed,
            now_unix_secs(),
        )
    }

    /// Builds a NIP-09 deletion request for an addressable discovery record.
    ///
    /// Names the record by its `a` coordinate (`kind:pubkey:d`) rather than by
    /// event id, so it covers whichever event currently occupies the slot
    /// without the caller having to have kept the id.
    pub fn create_discovery_deletion(keypair: &NostrKeypair, discovery_tag: &str) -> Result<Self> {
        let coordinate = format!(
            "{}:{}:{}",
            NOSTR_DISCOVERY_KIND,
            keypair.public_key_hex(),
            discovery_tag
        );
        let tags = vec![vec!["a".to_string(), coordinate]];
        Self::sign_with_tags(keypair, NOSTR_DELETION_KIND, tags, "", now_unix_secs())
    }

    /// Builds and signs a NIP-01 event with a single `p` tag.
    fn sign(
        keypair: &NostrKeypair,
        kind: u32,
        recipient_pubkey_hex: &str,
        content: &str,
        created_at: i64,
    ) -> Result<Self> {
        let tags = vec![vec!["p".to_string(), recipient_pubkey_hex.to_string()]];
        Self::sign_with_tags(keypair, kind, tags, content, created_at)
    }

    /// Builds and signs a NIP-01 event with caller-supplied tags.
    fn sign_with_tags(
        keypair: &NostrKeypair,
        kind: u32,
        tags: Vec<Vec<String>>,
        content: &str,
        created_at: i64,
    ) -> Result<Self> {
        let pubkey = keypair.public_key_hex().to_string();

        // NIP-01 canonical serialization for event ID computation:
        // [0, <pubkey>, <created_at>, <kind>, <tags_json>, <content>]
        let tags_json =
            serde_json::to_string(&tags).map_err(|e| Error::SerializationError(e.to_string()))?;
        let content_escaped =
            serde_json::to_string(content).map_err(|e| Error::SerializationError(e.to_string()))?;
        let serialized = format!(
            "[0,\"{}\",{},{},{},{}]",
            pubkey, created_at, kind, tags_json, content_escaped
        );

        // Event ID = SHA-256 of the serialized event
        let event_id_bytes = Sha256::digest(serialized.as_bytes());
        let event_id = hex::encode(event_id_bytes);

        // BIP-340 Schnorr signature of the event ID
        let sig = sign_event_id(&keypair.signing_key, &event_id_bytes)?;

        Ok(Self {
            id: event_id,
            pubkey,
            created_at,
            kind,
            tags,
            content: content.to_string(),
            sig,
        })
    }

    /// Serializes to the relay `["EVENT", {...}]` message format.
    pub fn to_relay_message(&self) -> Result<String> {
        let event_json = self.to_event_json()?;
        Ok(format!("[\"EVENT\",{}]", event_json))
    }

    /// Serializes just the event object (without the `["EVENT", ...]` wrapper).
    fn to_event_json(&self) -> Result<String> {
        // Use serde_json for safe serialization (no injection via field values).
        // Field order matches NIP-01 convention; most relays accept any order.
        let obj = serde_json::json!({
            "id": self.id,
            "pubkey": self.pubkey,
            "created_at": self.created_at,
            "kind": self.kind,
            "tags": self.tags,
            "content": self.content,
            "sig": self.sig,
        });
        serde_json::to_string(&obj).map_err(|e| Error::SerializationError(e.to_string()))
    }
}

/// Creates a NIP-01 REQ subscription message for DMs addressed to `pubkey_hex`
/// (this device's own routing tag, from [`routing_tag_for_address`]).
///
/// Returns
/// `["REQ", "<sub_id>", {"#p": ["<pubkey>"], "kinds": [4, 1059], "since": T, "limit": N}]`.
///
/// **Both kinds are requested, permanently.** [`NOSTR_GIFT_WRAP_KIND`] carries
/// sealed traffic and is what this device publishes; [`NOSTR_LEGACY_DM_KIND`]
/// is what peers that predate sealing publish, and dropping it from the filter
/// would make those peers silently undeliverable rather than merely
/// unprotected. The receive path distinguishes the two by payload shape, not by
/// the kind, so nothing downstream depends on this list.
///
/// The two bounds do different jobs and neither replaces the other:
///
/// - `since` (unix seconds, **inclusive** per NIP-01) is the real bound on
///   replay: it says how far back history may reach at all. The caller derives
///   it from the persisted receive watermark — see
///   [`NostrTransport::create_subscription`](crate::nostr::NostrTransport::create_subscription).
/// - [`NOSTR_INITIAL_QUERY_LIMIT`] caps how much of that window one initial
///   query may return. It is advisory in both directions (`limit` is a SHOULD,
///   and NIP-11 `max_limit` lets a relay clamp it silently), so it bounds the
///   worst case without being something to reason from.
///
/// Because `since` is inclusive, the event that set the watermark is returned
/// again on the next connect. That duplicate is expected; the alternative
/// (`since + 1`) would drop any event sharing that exact second.
///
/// **How much of the replayed overlap dedup actually absorbs is bounded, and
/// the bound is smaller than the overlap.** The engine's deduplicator retains
/// ids for `DeduplicatorConfig::retention_time_secs` (1 h by default) and at
/// most `max_tracked_messages` of them, while `since` reaches
/// `NOSTR_CREATED_AT_JITTER_SECS + NOSTR_CLOCK_SKEW_MARGIN_SECS` (1 h 5 min)
/// below the mark. A reconnect after longer than the retention window — the
/// ordinary case for a mobile app reopened the next day — therefore re-processes
/// the overlap rather than deduplicating it. That is a cost, not a loss: a
/// replayed ciphertext whose ratchet generation is spent fails closed as
/// `Decryption` and is dropped, a past-epoch one triggers at most one
/// rate-limited re-key, and a group copy TTLs out of the pending buffer. The
/// engine pins this relationship in
/// `nostr_replay_overlap_exceeds_dedup_retention` so the two constants cannot
/// drift apart from this note.
pub fn create_subscription_message(
    pubkey_hex: &str,
    subscription_id: &str,
    since: i64,
) -> Result<String> {
    let filter = serde_json::json!({
        "#p": [pubkey_hex],
        "kinds": [NOSTR_LEGACY_DM_KIND, NOSTR_GIFT_WRAP_KIND],
        "since": since.max(0),
        "limit": NOSTR_INITIAL_QUERY_LIMIT
    });
    let msg = serde_json::json!(["REQ", subscription_id, filter]);
    serde_json::to_string(&msg).map_err(|e| Error::SerializationError(e.to_string()))
}

/// Unseals a gift wrap's payload with one of our own keys.
///
/// - `recipient_key`: the key to attempt — our per-install signing key for
///   steady-state traffic, or the publicly computable one for the bootstrap leg.
/// - `sender_ephemeral_pubkey_hex`: the wrapper event's `pubkey` field, which is
///   the sender's single-use key. It is the *only* input to the shared secret;
///   it carries no identity, and the sender named inside the plaintext is the
///   one the protocol layer authenticates.
/// - `sealed`: the base64-decoded event `content` (the raw NIP-44 payload
///   bytes), which is the form the platform bridges hand us.
///
/// Failure is expected and cheap: the caller tries each candidate key in turn
/// and a wrong one is rejected by the MAC.
pub(crate) fn unwrap_gift_wrap(
    recipient_key: &NostrKeypair,
    sender_ephemeral_pubkey_hex: &str,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    let sender = hex::decode(sender_ephemeral_pubkey_hex)
        .map_err(|e| Error::CryptoError(format!("Invalid sender pubkey: {}", e)))?;
    let conversation_key = ConversationKey::derive(&recipient_key.signing_key, &sender)?;
    nip44::decrypt_bytes(sealed, &conversation_key)
}

/// Opens a peer's published key-package record.
///
/// Mechanically identical to [`unwrap_gift_wrap`] — NIP-44's conversation key
/// is symmetric, so the same derive-and-decrypt serves both — but the key here
/// is the **peer's** publicly computable one, reconstructed from their user id,
/// not one of ours. That inversion is the reason this has its own name: a
/// reader who found `unwrap_gift_wrap` being handed someone else's key would be
/// right to read it as a bug.
///
/// Note what is deliberately absent: the event's BIP-340 signature is never
/// checked. It would prove only that whoever published the record holds the key
/// the record names, which is not a claim anything relies on — the peer's real
/// Nostr key is taken from the Ed25519-signed payload inside, under the same
/// rule the sealed-envelope work established. Verifying it would buy a
/// reassuring-looking check that authenticates nothing.
pub(crate) fn open_key_package_publication(
    peer_derivable_key: &NostrKeypair,
    author_pubkey_hex: &str,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    unwrap_gift_wrap(peer_derivable_key, author_pubkey_hex, sealed)
}

/// Builds the REQ that fetches a peer's published key-package records.
///
/// Deliberately carries no `since`: the receive watermark bounds the *message*
/// subscription, where re-reading history is waste, but a key-package record is
/// republished only when a slot is consumed and may therefore be arbitrarily
/// old while still being current. A `since` here would hide exactly the records
/// belonging to a stable peer who has needed no refresh.
///
/// `limit` is the slot count rather than [`NOSTR_INITIAL_QUERY_LIMIT`]: a
/// well-behaved peer publishes that many addressable events and no more, so
/// anything beyond it is another author crowding the same tag.
pub(crate) fn create_key_package_query_message(
    routing_tag: &str,
    subscription_id: &str,
) -> Result<String> {
    let filter = serde_json::json!({
        "#p": [routing_tag],
        "kinds": [NOSTR_KEY_PACKAGE_KIND],
        "limit": crate::constants::NOSTR_KEY_PACKAGE_SLOTS
    });
    let msg = serde_json::json!(["REQ", subscription_id, filter]);
    serde_json::to_string(&msg).map_err(|e| Error::SerializationError(e.to_string()))
}

/// Opens a sealed discovery record.
///
/// Like [`open_key_package_publication`], the *unsealing* does not authenticate
/// the publisher: it proves only that the content was sealed to the discovery
/// key, which is public by construction. Authenticity of a claim comes from the
/// Ed25519 signature inside it, and the binding of that claim to this publisher
/// from comparing the record's `nostr_author` against the event's author.
///
/// A **tombstone has no inside**, which is why discovery events must
/// additionally pass [`event_is_authentic`] before reaching here. See that
/// function.
pub(crate) fn open_discovery_record(
    discovery_seal_key: &NostrKeypair,
    author_pubkey_hex: &str,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    unwrap_gift_wrap(discovery_seal_key, author_pubkey_hex, sealed)
}

/// Whether an inbound event is genuinely from the key it names: its `id` is the
/// NIP-01 hash of its own fields, and its `sig` is a BIP-340 signature over
/// that id under its `pubkey`.
///
/// # Why a discovery record needs this and a key package does not
///
/// Everywhere else in this transport the event signature is worthless: it
/// proves only that the publisher holds the key the event names, which is a
/// claim nothing rests on, because every record carries its own inner Ed25519
/// signature and that is what is verified.
///
/// A **tombstone breaks that symmetry**. Its whole body is
/// `{"v":1,"retracted":true}` — there is nothing inside to sign, and its entire
/// meaning comes from *who published it*. The discovery seal key is public by
/// construction ([`discovery_seal_keypair_for_username`]), so anyone who knows
/// the username can derive the conversation key for **any** author and seal a
/// tombstone attributed to them. Without this check, a single hostile relay
/// could serve a forged tombstone for an honest claimant and suppress their
/// claim from the resolved set — even while every other relay served the
/// genuine record. Resolution is order-independent and a tombstone is sticky
/// for the life of the resolution (deliberately, so a stale copy cannot stand a
/// retracted claim back up), which is exactly what makes the forgery total
/// rather than racy.
///
/// That inverts what multiple relays are for. A claim needs *one* honest relay
/// to survive; without this check a retraction needed only *one* hostile relay
/// to succeed. A squatter running a popular relay could then make their own
/// claim the only one a user ever sees, which is the authoritative-looking
/// directory the whole set-shaped API exists to prevent.
///
/// # The id is recomputed, never trusted
///
/// The `id` field is re-derived from the event's own fields and compared before
/// the signature is checked. Trusting the claimed id would leave a second hole:
/// the resolver takes each event id once per query, so an event claiming a
/// genuine record's id could consume its dedup slot and have the real record
/// dropped as a duplicate.
///
/// Returns `false` for anything malformed. Junk at a public tag is ordinary,
/// not exceptional.
pub(crate) fn event_is_authentic(event: &serde_json::Value) -> bool {
    use k256::ecdsa::signature::hazmat::PrehashVerifier;
    use k256::schnorr::VerifyingKey;

    let (
        Some(id),
        Some(pubkey),
        Some(created_at),
        Some(kind),
        Some(tags),
        Some(content),
        Some(sig),
    ) = (
        event.get("id").and_then(|v| v.as_str()),
        event.get("pubkey").and_then(|v| v.as_str()),
        event.get("created_at").and_then(|v| v.as_i64()),
        event.get("kind").and_then(|v| v.as_u64()),
        event.get("tags"),
        event.get("content").and_then(|v| v.as_str()),
        event.get("sig").and_then(|v| v.as_str()),
    )
    else {
        return false;
    };

    // Decoded before it is interpolated below: this both rejects a key no
    // curve can accept and guarantees the string is pure hex, so it cannot
    // carry a quote into the canonical serialization. The length check is
    // load-bearing for a second reason — `VerifyingKey::from_bytes` *panics*
    // on any other length rather than erroring, and this value comes straight
    // off a public tag. See `ConversationKey::derive`.
    let Ok(pubkey_bytes) = hex::decode(pubkey) else {
        return false;
    };
    if pubkey_bytes.len() != 32 {
        return false;
    }

    let (Ok(tags_json), Ok(content_escaped)) =
        (serde_json::to_string(tags), serde_json::to_string(content))
    else {
        return false;
    };
    // The same NIP-01 canonical serialization `sign_with_tags` produces, which
    // is what keeps a record this build published verifiable by this build.
    let serialized = format!(
        "[0,\"{}\",{},{},{},{}]",
        pubkey, created_at, kind, tags_json, content_escaped
    );
    let recomputed = Sha256::digest(serialized.as_bytes());
    if !id.eq_ignore_ascii_case(&hex::encode(recomputed)) {
        return false;
    }

    let Ok(vk) = VerifyingKey::from_bytes(&pubkey_bytes) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(sig) else {
        return false;
    };
    let Ok(signature) = k256::schnorr::Signature::try_from(sig_bytes.as_slice()) else {
        return false;
    };
    vk.verify_prehash(&recomputed, &signature).is_ok()
}

/// Builds the REQ that fetches the records claiming a username.
///
/// Carries no `since`, for the same reason the key-package query does not: a
/// claim is republished only when it changes, so a stable claimant's record may
/// be arbitrarily old while remaining entirely current. A `since` here would
/// hide exactly the long-settled claims a directory exists to return.
///
/// `limit` bounds how many claimants one relay may offer. It is well above the
/// device count of any real user and far below
/// [`MAX_QUERY_EVENTS`](crate::nostr::MAX_QUERY_EVENTS): the tag is public, so
/// anything past a handful of records is another author crowding it, and
/// crowding costs the *legitimate* claimants their place in the answer.
pub(crate) fn create_discovery_query_message(
    discovery_tag: &str,
    subscription_id: &str,
) -> Result<String> {
    let filter = serde_json::json!({
        "#p": [discovery_tag],
        "kinds": [NOSTR_DISCOVERY_KIND],
        "limit": crate::constants::NOSTR_DISCOVERY_QUERY_LIMIT
    });
    let msg = serde_json::json!(["REQ", subscription_id, filter]);
    serde_json::to_string(&msg).map_err(|e| Error::SerializationError(e.to_string()))
}

/// Whether `data` has the shape of a sealed gift-wrap payload.
pub(crate) fn is_sealed_payload(data: &[u8]) -> bool {
    nip44::looks_like_payload(data)
}

/// BIP-340 Schnorr signature of a 32-byte event ID hash.
fn sign_event_id(signing_key: &SigningKey, event_id_hash: &[u8]) -> Result<String> {
    use k256::ecdsa::signature::hazmat::PrehashSigner;
    let signature: k256::schnorr::Signature = signing_key
        .sign_prehash(event_id_hash)
        .map_err(|e| Error::CryptoError(format!("Schnorr signing failed: {}", e)))?;
    Ok(hex::encode(signature.to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// The address a test label stands for.
    ///
    /// Both derivations here take an address, and an address is not a name —
    /// writing `routing_tag_for_address("bob")` would still compute *a* tag,
    /// and would quietly teach the next reader that usernames are valid input.
    fn addr(label: &str) -> Address {
        let digest = Sha256::digest(label.as_bytes());
        let mut hash = [0u8; Address::HASH_LEN];
        hash.copy_from_slice(&digest[..Address::HASH_LEN]);
        Address::from_hash_bytes(hash)
    }

    fn user(label: &str) -> Username {
        label.parse().expect("username should parse")
    }

    /// The discovery tag and the *address* routing tag for the same string must
    /// differ. This is the whole point of domain-separating the discovery
    /// preimage (see [`discovery_tag_for_username`]), and it is the case a
    /// naive implementation collapses by hashing the bare name.
    ///
    /// The comparison is made over a string that is simultaneously a valid
    /// username and, as a label, derivable to an address — so the two
    /// derivations are being fed the same bytes, which is exactly the
    /// collision this separation prevents.
    #[test]
    fn test_discovery_tag_is_not_the_address_routing_tag() {
        for label in ["alice", "bob", "carol"] {
            let username = user(label);
            let discovery = discovery_tag_for_username(&username).expect("discovery tag");

            // The address whose *canonical string* is what the routing tag
            // hashes. Different preimage spaces entirely, which is the point.
            let address = addr(label);
            let routing = routing_tag_for_address(&address).expect("routing tag");

            assert_ne!(
                discovery, routing,
                "the discovery tag must not equal the routing tag for {label}"
            );
        }
    }

    /// A username and an address that render to the same *string* must still
    /// derive different tags. Address-shaped usernames are refused by the
    /// `Username` type, so this pins the property one level down: even if that
    /// screen were removed, the domain separator alone keeps the namespaces
    /// apart.
    #[test]
    fn test_discovery_tag_domain_separates_from_the_bare_hash() {
        let username = user("alice");
        let tag = discovery_tag_for_username(&username).expect("tag");

        // What the tag would have been without the domain separator.
        let bare_scalar = Sha256::digest(b"alice");
        let bare = hex::encode(
            SigningKey::from_bytes(bare_scalar.as_slice())
                .expect("scalar")
                .verifying_key()
                .to_bytes(),
        );

        assert_ne!(
            tag, bare,
            "the discovery tag must not be the undomained hash of the username"
        );
    }

    /// The same invariant [`test_record_seal_key_is_not_the_routing_tag`] pins
    /// for addresses. A seal key that *is* the tag leaves every published label
    /// standing as a key whose private half anyone can compute, which stops
    /// being harmless the moment something reaches for "the key matching this
    /// tag".
    #[test]
    fn test_discovery_seal_key_is_not_the_discovery_tag() {
        for label in ["alice", "bob", "a-name-with-dashes"] {
            let username = user(label);
            assert_ne!(
                discovery_seal_keypair_for_username(&username)
                    .unwrap()
                    .public_key_hex(),
                discovery_tag_for_username(&username).unwrap(),
                "the discovery-seal key must not be the discovery tag for {label}"
            );
        }
    }

    /// The two seal keys must not collide either: a username and an address are
    /// different namespaces and their seals are derived under different HKDF
    /// info strings.
    #[test]
    fn test_discovery_seal_key_is_not_the_record_seal_key() {
        for label in ["alice", "bob"] {
            let username = user(label);
            let address = addr(label);
            assert_ne!(
                discovery_seal_keypair_for_username(&username)
                    .unwrap()
                    .public_key_hex(),
                record_seal_keypair_for_address(&address)
                    .unwrap()
                    .public_key_hex(),
                "the discovery-seal key must not be the record-seal key for {label}"
            );
        }
    }

    #[test]
    fn test_discovery_tag_and_seal_key_are_deterministic() {
        let username = user("alice");
        assert_eq!(
            discovery_tag_for_username(&username).unwrap(),
            discovery_tag_for_username(&username).unwrap()
        );
        assert_eq!(
            discovery_seal_keypair_for_username(&username)
                .unwrap()
                .public_key_hex(),
            discovery_seal_keypair_for_username(&username)
                .unwrap()
                .public_key_hex()
        );
    }

    #[test]
    fn test_discovery_tag_is_username_specific() {
        assert_ne!(
            discovery_tag_for_username(&user("alice")).unwrap(),
            discovery_tag_for_username(&user("bob")).unwrap()
        );
    }

    /// Golden vectors for the discovery derivations, so a second
    /// implementation can check its tag and seal derivations without this
    /// repository.
    ///
    /// **Computed independently of this code**, by a Python script that
    /// implements secp256k1 from the curve equation and HKDF-SHA256 from
    /// RFC 5869 — not by calling `k256` or the `hkdf` crate. That script was
    /// itself validated by reproducing all three shipped
    /// [`test_routing_tag_golden_values`] literals, which were generated
    /// independently of it, so the arithmetic behind these values is
    /// cross-checked rather than self-consistent.
    ///
    /// Every intermediate is pinned as well as the result: a mismatch then
    /// says *which* step diverged — the preimage, the HKDF, or the curve —
    /// instead of only that the final hex differs.
    #[test]
    fn test_discovery_derivation_golden_values() {
        let username = user("alice");

        // The part another implementation is most likely to get wrong: the
        // separator, and the fact that the name is the *normalized* one.
        assert_eq!(
            hex::encode(Sha256::digest(b"offline-disc-v1:alice")),
            "f0fe1d050a281d611fdd951f13e6e6878407ec87a80cd7fa0b42442f44020466"
        );

        assert_eq!(
            discovery_tag_for_username(&username).unwrap(),
            "179183d1f394327bb8a244dbe9160dd77bf86f7125caa21c2d2eec6e50267703"
        );
        assert_eq!(
            discovery_seal_keypair_for_username(&username)
                .unwrap()
                .public_key_hex(),
            "a008efac2c8ebe103dc2dfc78acd7890cff5cadb238159eaf615aa7ad9d8994e"
        );
    }

    /// A discovery publication is addressable at a *deterministic* `d`, which
    /// is what makes republishing replace the claim rather than accumulate
    /// claims. Pinned because the key-package record does the opposite, and
    /// copying its random slot id here would silently break retraction.
    #[test]
    fn test_discovery_publication_is_addressable_at_the_tag() {
        let keypair = NostrKeypair::generate_ephemeral().expect("keypair");
        let username = user("alice");
        let tag = discovery_tag_for_username(&username).expect("tag");
        let seal = discovery_seal_keypair_for_username(&username).expect("seal");

        let event =
            NostrEvent::create_discovery_publication(&keypair, &tag, seal.public_key_hex(), b"{}")
                .expect("publication");

        assert_eq!(event.kind, NOSTR_DISCOVERY_KIND);
        let d_tag = event
            .tags
            .iter()
            .find(|t| t.first().map(String::as_str) == Some("d"))
            .expect("d tag");
        assert_eq!(d_tag[1], tag, "the d tag must be the discovery tag");
        let p_tag = event
            .tags
            .iter()
            .find(|t| t.first().map(String::as_str) == Some("p"))
            .expect("p tag");
        assert_eq!(p_tag[1], tag);
    }

    /// Republishing must land on the same addressable coordinate, or the relay
    /// keeps both and a retraction never displaces the claim it retracts.
    #[test]
    fn test_discovery_republication_replaces_rather_than_accumulates() {
        let keypair = NostrKeypair::generate_ephemeral().expect("keypair");
        let tag = discovery_tag_for_username(&user("alice")).expect("tag");
        let seal = discovery_seal_keypair_for_username(&user("alice")).expect("seal");

        let first =
            NostrEvent::create_discovery_publication(&keypair, &tag, seal.public_key_hex(), b"{}")
                .expect("first");
        let second = NostrEvent::create_discovery_publication(
            &keypair,
            &tag,
            seal.public_key_hex(),
            b"{\"v\":1}",
        )
        .expect("second");

        assert_eq!(first.kind, second.kind);
        assert_eq!(first.pubkey, second.pubkey);
        let d_of = |e: &NostrEvent| {
            e.tags
                .iter()
                .find(|t| t.first().map(String::as_str) == Some("d"))
                .expect("d tag")[1]
                .clone()
        };
        assert_eq!(d_of(&first), d_of(&second));
    }

    /// `created_at` must be the true present, never jittered into the past:
    /// relays keep the newest event per `(kind, pubkey, d)`, so a backdated
    /// republication is dropped and strands the claim it meant to replace.
    #[test]
    fn test_discovery_publication_is_not_backdated() {
        let keypair = NostrKeypair::generate_ephemeral().expect("keypair");
        let tag = discovery_tag_for_username(&user("alice")).expect("tag");
        let seal = discovery_seal_keypair_for_username(&user("alice")).expect("seal");

        let before = now_unix_secs();
        let event =
            NostrEvent::create_discovery_publication(&keypair, &tag, seal.public_key_hex(), b"{}")
                .expect("publication");
        let after = now_unix_secs();

        assert!(
            event.created_at >= before && event.created_at <= after,
            "created_at {} must sit in [{}, {}]",
            event.created_at,
            before,
            after
        );
    }

    #[test]
    fn test_discovery_record_round_trips_through_its_seal() {
        let keypair = NostrKeypair::generate_ephemeral().expect("keypair");
        let username = user("alice");
        let tag = discovery_tag_for_username(&username).expect("tag");
        let seal = discovery_seal_keypair_for_username(&username).expect("seal");
        let plaintext = b"{\"v\":1,\"username\":\"alice\"}";

        let event = NostrEvent::create_discovery_publication(
            &keypair,
            &tag,
            seal.public_key_hex(),
            plaintext,
        )
        .expect("publication");

        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .expect("content is base64");
        let opened = open_discovery_record(&seal, &event.pubkey, &sealed).expect("open");
        assert_eq!(opened, plaintext);
    }

    /// Anyone who knows the username can open the record. That is the design,
    /// not a leak — the payload holds only what is public to someone who
    /// already knows the name — and it is pinned so a later change that makes
    /// this key load-bearing fails a test rather than a review.
    #[test]
    fn test_discovery_seal_is_openable_by_anyone_who_knows_the_username() {
        let keypair = NostrKeypair::generate_ephemeral().expect("keypair");
        let username = user("alice");
        let tag = discovery_tag_for_username(&username).expect("tag");
        let publisher_seal = discovery_seal_keypair_for_username(&username).expect("seal");

        let event = NostrEvent::create_discovery_publication(
            &keypair,
            &tag,
            publisher_seal.public_key_hex(),
            b"public",
        )
        .expect("publication");

        // A stranger reconstructs the same key from the name alone.
        let stranger_seal = discovery_seal_keypair_for_username(&user("alice")).expect("seal");
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .expect("base64");
        assert_eq!(
            open_discovery_record(&stranger_seal, &event.pubkey, &sealed).expect("open"),
            b"public"
        );
    }

    #[test]
    fn test_discovery_query_names_the_right_kind_and_tag() {
        let tag = discovery_tag_for_username(&user("alice")).expect("tag");
        let req = create_discovery_query_message(&tag, "sub1").expect("req");
        let parsed: serde_json::Value = serde_json::from_str(&req).expect("json");

        assert_eq!(parsed[0], "REQ");
        assert_eq!(parsed[1], "sub1");
        assert_eq!(parsed[2]["kinds"][0], NOSTR_DISCOVERY_KIND);
        assert_eq!(parsed[2]["#p"][0], tag);
        assert_eq!(
            parsed[2]["limit"],
            crate::constants::NOSTR_DISCOVERY_QUERY_LIMIT
        );
        assert!(
            parsed[2].get("since").is_none(),
            "a discovery query must carry no `since`: a settled claim is old \
             and current at the same time"
        );
    }

    #[test]
    fn test_discovery_deletion_names_the_addressable_coordinate() {
        let keypair = NostrKeypair::generate_ephemeral().expect("keypair");
        let tag = discovery_tag_for_username(&user("alice")).expect("tag");
        let event = NostrEvent::create_discovery_deletion(&keypair, &tag).expect("deletion");

        assert_eq!(event.kind, NOSTR_DELETION_KIND);
        let a_tag = event
            .tags
            .iter()
            .find(|t| t.first().map(String::as_str) == Some("a"))
            .expect("a tag");
        assert_eq!(
            a_tag[1],
            format!(
                "{}:{}:{}",
                NOSTR_DISCOVERY_KIND,
                keypair.public_key_hex(),
                tag
            )
        );
    }

    #[test]
    fn test_from_install_secret_deterministic() {
        let secret = [7u8; 32];
        let kp = NostrKeypair::from_install_secret(&secret).unwrap();
        assert_eq!(kp.public_key_hex().len(), 64);
        // Deterministic: same secret → same key across "restarts"
        let kp2 = NostrKeypair::from_install_secret(&secret).unwrap();
        assert_eq!(kp.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_different_secrets_different_keys() {
        let kp1 = NostrKeypair::from_install_secret(&[1u8; 32]).unwrap();
        let kp2 = NostrKeypair::from_install_secret(&[2u8; 32]).unwrap();
        assert_ne!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_from_install_secret_rejects_short_secret() {
        assert!(NostrKeypair::from_install_secret(&[0u8; 15]).is_err());
        assert!(NostrKeypair::from_install_secret(b"").is_err());
    }

    #[test]
    fn test_signing_key_is_not_derivable_from_the_address() {
        // The oldest invariant in this file, kept because the thing it guards
        // against is the whole reason the key model has three entries: a
        // signing key anyone can recompute from a public identifier is not an
        // identity. Feeding the address straight into the install-secret
        // derivation must not land on either public value.
        let address = addr("alice");
        let secret = Sha256::digest(address.to_string().as_bytes());
        let kp = NostrKeypair::from_install_secret(secret.as_slice()).unwrap();

        assert_ne!(
            kp.public_key_hex(),
            routing_tag_for_address(&address).unwrap()
        );
        assert_ne!(
            kp.public_key_hex(),
            record_seal_keypair_for_address(&address)
                .unwrap()
                .public_key_hex()
        );
    }

    #[test]
    fn test_generate_ephemeral_is_random() {
        let kp1 = NostrKeypair::generate_ephemeral().unwrap();
        let kp2 = NostrKeypair::generate_ephemeral().unwrap();
        assert_ne!(kp1.public_key_hex(), kp2.public_key_hex());
    }

    #[test]
    fn test_generate_install_secret_is_random() {
        let s1 = NostrKeypair::generate_install_secret().unwrap();
        let s2 = NostrKeypair::generate_install_secret().unwrap();
        assert_ne!(*s1, *s2);
        assert_ne!(*s1, [0u8; 32]);
    }

    #[test]
    fn test_routing_tag_golden_values() {
        // Re-pinned on addresses. The previous vectors were computed from
        // usernames and carried a comment saying they "must never change, or
        // addressing breaks against deployed peers" — which was true when the
        // preimage was a username, and stopped being true twice over: the
        // preimage is now the derived address, and the identity migration is a
        // deliberate clean cutover with no fleet to break.
        //
        // So this is no longer a backward-compatibility contract. What it
        // still guards is that two builds computing a tag for the same address
        // agree — the failure it catches is a silent one, where both sides
        // think they are talking and each subscribes to a label the other
        // never writes to.
        //
        // Written out as literal addresses rather than through `addr()`: a
        // golden vector that derives its own input can only ever agree with
        // itself.
        //
        // Each literal is parsed via `vector_address`, which is what makes a
        // typo'd vector a failure instead of a silent re-pinning. While these
        // took `&str` a corrupted literal still hashed to *something*, so the
        // test kept passing against a preimage no runtime path could ever
        // produce — the vectors agreed with themselves about a value nobody
        // would compute.
        assert_eq!(
            routing_tag_for_address(&vector_address(
                "off1qy4aspkf0u8qptc6rlpn9ra8vw5jd9ereq4cwpfs"
            ))
            .unwrap(),
            "2ba510b01e5a0f1a76ed8e66beb430642960e740aedf7d8f1c8b21cb11028fc2"
        );
        assert_eq!(
            routing_tag_for_address(&vector_address(
                "off1qxqmvd7clnfvdknrt8nfvvgn5ytsmeu4us5lrr0g"
            ))
            .unwrap(),
            "32d26c7b8fbbb9268aba57a45d84a5554cc23e880aef5a7b2ce06e490a1ba35c"
        );
        assert_eq!(
            routing_tag_for_address(&vector_address(
                "off1qyv04gxa02f8jpkt8cu06nlk3x0mu35wdqyxt6jq"
            ))
            .unwrap(),
            "e7505f56a2de5a5d3946949ab58bde530b452f46f4c11f85130c18e48b7bbd51"
        );
    }

    /// Parses a golden-vector literal, failing loudly if it is not a valid
    /// address.
    ///
    /// The vectors pin a derivation whose *input* is the thing that has to be
    /// right. A literal that is not a real address pins a tag no peer can
    /// produce, which is worse than no vector at all — it reads as coverage.
    fn vector_address(literal: &str) -> Address {
        literal
            .parse::<Address>()
            .unwrap_or_else(|e| panic!("golden vector '{literal}' is not a valid address: {e}"))
    }

    /// The record-seal key is a wire contract in the same way the tag is: a
    /// publisher seals with it and a fetcher, on some other build, reconstructs
    /// it independently. If the two derivations ever disagree, every published
    /// record silently stops opening.
    #[test]
    fn test_record_seal_key_golden_values() {
        assert_eq!(
            record_seal_keypair_for_address(&vector_address(
                "off1qy4aspkf0u8qptc6rlpn9ra8vw5jd9ereq4cwpfs"
            ))
            .unwrap()
            .public_key_hex(),
            "62f2835dc8788282d7ce92864fd1819b12b472e209b0116c17a1d06a5f9eebb0"
        );
        assert_eq!(
            record_seal_keypair_for_address(&vector_address(
                "off1qxqmvd7clnfvdknrt8nfvvgn5ytsmeu4us5lrr0g"
            ))
            .unwrap()
            .public_key_hex(),
            "c6c428f62e777b97c29a8e1a8d7fb2aaaa14bf55e5adf3ced4ed30a5c3c27755"
        );
    }

    #[test]
    fn test_create_dm_event() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_address(&addr("bob")).unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();

        assert_eq!(event.id.len(), 64); // 32 bytes hex
        assert_eq!(event.pubkey, sender.public_key_hex());
        assert_eq!(event.kind, 4);
        assert_eq!(event.tags.len(), 1);
        assert_eq!(event.tags[0][0], "p");
        assert_eq!(event.tags[0][1], recipient_pubkey);
        assert_eq!(event.content, "dGVzdCBtZXNzYWdl");
        assert_eq!(event.sig.len(), 128); // 64 bytes hex
    }

    #[test]
    fn test_event_id_is_sha256_of_serialization() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_address(&addr("bob")).unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();

        // Recompute the event ID from the canonical serialization
        let tags_json = serde_json::to_string(&event.tags).unwrap();
        let content_escaped = serde_json::to_string(&event.content).unwrap();
        let serialized = format!(
            "[0,\"{}\",{},{},{},{}]",
            event.pubkey, event.created_at, event.kind, tags_json, content_escaped
        );
        let expected_id = hex::encode(Sha256::digest(serialized.as_bytes()));
        assert_eq!(event.id, expected_id);
    }

    #[test]
    fn test_signature_verification() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_address(&addr("bob")).unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();

        // Verify the signature
        let verifying_key = sender.signing_key.verifying_key();
        let event_id_bytes = hex::decode(&event.id).unwrap();
        let sig_bytes = hex::decode(&event.sig).unwrap();
        let signature = k256::schnorr::Signature::try_from(sig_bytes.as_slice()).unwrap();

        use k256::ecdsa::signature::hazmat::PrehashVerifier;
        verifying_key
            .verify_prehash(&event_id_bytes, &signature)
            .expect("Signature should be valid");
    }

    #[test]
    fn test_to_relay_message() {
        let sender = NostrKeypair::generate_ephemeral().unwrap();
        let recipient_pubkey = routing_tag_for_address(&addr("bob")).unwrap();

        let event = NostrEvent::create_dm(&sender, &recipient_pubkey, "dGVzdCBtZXNzYWdl").unwrap();
        let relay_msg = event.to_relay_message().unwrap();

        assert!(relay_msg.starts_with("[\"EVENT\",{"));
        assert!(relay_msg.ends_with("}]"));
        assert!(relay_msg.contains(&format!("\"id\":\"{}\"", event.id)));
        assert!(relay_msg.contains(&format!("\"sig\":\"{}\"", event.sig)));
    }

    #[test]
    fn test_create_subscription_message() {
        let pubkey = routing_tag_for_address(&addr("alice")).unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        assert!(msg.starts_with("[\"REQ\",\"sub123\",{"));
        assert!(msg.contains(&format!("\"#p\":[\"{}\"]", pubkey)));
        assert!(msg.contains(&format!(
            "\"kinds\":[{},{}]",
            NOSTR_LEGACY_DM_KIND, NOSTR_GIFT_WRAP_KIND
        )));
    }

    #[test]
    fn test_subscription_keeps_requesting_the_legacy_dm_kind() {
        // Sealed traffic is published as kind 1059, but a peer running a build
        // from before sealing still publishes kind 4. Dropping kind 4 from the
        // filter would make those peers silently undeliverable — no error, no
        // event, just nothing arriving — so both kinds stay requested.
        let pubkey = routing_tag_for_address(&addr("alice")).unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let kinds: Vec<u64> = parsed[2]["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_u64().unwrap())
            .collect();
        assert!(kinds.contains(&u64::from(NOSTR_LEGACY_DM_KIND)));
        assert!(kinds.contains(&u64::from(NOSTR_GIFT_WRAP_KIND)));
    }

    #[test]
    fn test_gift_wrap_is_signed_by_a_single_use_key() {
        // The wrapper's signing key must be fresh per event: a stable one would
        // let a relay group every message this device ever publishes, which is
        // most of the metadata that sealing exists to remove.
        let bob_tag = routing_tag_for_address(&addr("bob")).unwrap();

        let a = NostrEvent::create_gift_wrap(&bob_tag, &bob_tag, b"hello").unwrap();
        let b = NostrEvent::create_gift_wrap(&bob_tag, &bob_tag, b"hello").unwrap();

        assert_eq!(a.kind, NOSTR_GIFT_WRAP_KIND);
        assert_ne!(
            a.pubkey, b.pubkey,
            "outer key must not repeat across events"
        );
        assert_ne!(a.content, b.content, "fresh nonce must change the payload");
        assert_eq!(a.tags[0], vec!["p".to_string(), bob_tag.clone()]);
    }

    #[test]
    fn test_gift_wrap_round_trips_to_the_recipients_key() {
        let bob = NostrKeypair::from_install_secret(&[77u8; 32]).unwrap();
        let bob_tag = routing_tag_for_address(&addr("bob")).unwrap();

        let event =
            NostrEvent::create_gift_wrap(&bob_tag, bob.public_key_hex(), b"payload").unwrap();
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .unwrap();

        assert_eq!(
            unwrap_gift_wrap(&bob, &event.pubkey, &sealed).unwrap(),
            b"payload"
        );

        // Sealed to Bob's install key, so Bob's *derivable* key must not open
        // it — otherwise the steady-state seal would be no better than the
        // bootstrap one.
        let bob_derivable = record_seal_keypair_for_address(&addr("bob")).unwrap();
        assert!(unwrap_gift_wrap(&bob_derivable, &event.pubkey, &sealed).is_err());
    }

    #[test]
    fn test_bootstrap_wrap_opens_with_the_record_seal_key() {
        // First contact: the sender knows only the recipient's address, so it
        // addresses the frame to their tag and seals it to their record-seal
        // key. Those are now two different values, which is the whole point of
        // keeping them as two arguments.
        let bob_tag = routing_tag_for_address(&addr("bob")).unwrap();
        let bob_seal = record_seal_keypair_for_address(&addr("bob")).unwrap();

        let event =
            NostrEvent::create_gift_wrap(&bob_tag, bob_seal.public_key_hex(), b"first contact")
                .unwrap();
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .unwrap();

        assert_eq!(
            unwrap_gift_wrap(&bob_seal, &event.pubkey, &sealed).unwrap(),
            b"first contact"
        );
        assert_eq!(
            event.tags[0],
            vec!["p".to_string(), bob_tag],
            "the frame is addressed to the tag even though it is sealed to the key"
        );
    }

    #[test]
    fn test_record_seal_key_is_not_the_routing_tag() {
        // These were the same value for most of this transport's life. Nothing
        // was broken by it, but it left every routing tag standing as a public
        // key whose private half anyone could compute — which stops being
        // harmless the moment something adds NIP-42 AUTH or pubkey filtering
        // and reaches for "the key matching this tag".
        //
        // Separate derivations, so there is no such key.
        for label in ["alice", "bob", "device1", "a-very-long-user-id-with-dashes"] {
            let address = addr(label);
            assert_ne!(
                record_seal_keypair_for_address(&address)
                    .unwrap()
                    .public_key_hex(),
                routing_tag_for_address(&address).unwrap(),
                "the record-seal key must not be the routing tag for {label}"
            );
        }
    }

    #[test]
    fn test_record_seal_key_is_deterministic_and_address_specific() {
        // Deterministic because both sides derive it independently — the
        // publisher to seal, a fetcher to open — and they never exchange it.
        assert_eq!(
            record_seal_keypair_for_address(&addr("alice"))
                .unwrap()
                .public_key_hex(),
            record_seal_keypair_for_address(&addr("alice"))
                .unwrap()
                .public_key_hex()
        );
        assert_ne!(
            record_seal_keypair_for_address(&addr("alice"))
                .unwrap()
                .public_key_hex(),
            record_seal_keypair_for_address(&addr("bob"))
                .unwrap()
                .public_key_hex()
        );
    }

    #[test]
    fn test_gift_wrap_created_at_is_jittered_into_the_past_only() {
        // NIP-59 requires past-only jitter. A future-dated wrapper would be
        // filtered out by peers' `until` bounds and, on our own receive path,
        // is exactly what `NOSTR_FUTURE_DATED_TOLERANCE_SECS` refuses to let
        // advance the watermark.
        let bob_tag = routing_tag_for_address(&addr("bob")).unwrap();
        let mut saw_jitter = false;

        for _ in 0..64 {
            let now = now_unix_secs();
            let event = NostrEvent::create_gift_wrap(&bob_tag, &bob_tag, b"x").unwrap();
            assert!(
                event.created_at <= now,
                "created_at must never be in the future: {} > {}",
                event.created_at,
                now
            );
            assert!(
                now - event.created_at <= NOSTR_CREATED_AT_JITTER_SECS,
                "created_at must stay inside the jitter window the subscription overlaps"
            );
            if now - event.created_at > 0 {
                saw_jitter = true;
            }
        }
        assert!(
            saw_jitter,
            "timestamps were never jittered across 64 events"
        );
    }

    #[test]
    fn test_subscription_filter_bounds_stored_event_replay() {
        // Without a `limit` the filter is unbounded, so every relay
        // (re)connect replays the relay's whole retention window — which
        // NIP-11 no longer advertises, so it cannot even be reasoned about.
        let pubkey = routing_tag_for_address(&addr("alice")).unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let filter = &parsed[2];
        assert_eq!(
            filter["limit"].as_u64(),
            Some(NOSTR_INITIAL_QUERY_LIMIT as u64),
            "REQ filter must carry a limit: {}",
            msg
        );
    }

    #[test]
    fn test_subscription_filter_carries_since() {
        let pubkey = routing_tag_for_address(&addr("alice")).unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", 1_700_000_000).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(
            parsed[2]["since"].as_i64(),
            Some(1_700_000_000),
            "REQ filter must carry the caller's since: {}",
            msg
        );
    }

    #[test]
    fn test_subscription_since_is_never_negative() {
        // A negative `since` is not a valid NIP-01 filter value; relays may
        // reject the whole REQ, which would take the subscription down rather
        // than merely widening it.
        let pubkey = routing_tag_for_address(&addr("alice")).unwrap();
        let msg = create_subscription_message(&pubkey, "sub123", -42).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed[2]["since"].as_i64(), Some(0));
    }
}
