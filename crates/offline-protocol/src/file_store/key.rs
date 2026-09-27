//! Where the sealed MLS file store gets its key.

use base64::Engine as _;
use zeroize::Zeroizing;

/// Length of a store key.
pub const STORE_KEY_BYTES: usize = 32;

/// Why a [`StoreKeyProvider`] produced no key.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum StoreKeyError {
    /// No key is configured where the provider looks.
    #[error("no store key is configured: {0}")]
    Missing(String),
    /// Something is configured, but it is not a usable key.
    #[error("the configured store key is not usable: {0}")]
    Malformed(String),
    /// The provider's source failed.
    #[error("the store key source failed: {0}")]
    Unavailable(String),
}

/// Supplies the store key that [`super::SealedFileMlsStorage`] seals under.
///
/// The store key protects the device's identity keys, its group state and
/// the record key that seals protocol state, so it is the one secret a
/// headless host has to keep. The SDK asks for it once, when the store opens,
/// derives its working keys from it and keeps nothing else.
///
/// A host with a keystore (a TPM, a secrets manager, a service manager's
/// credential store) implements this trait over it. Without one, the key
/// arrives in the clear from wherever the operator put it; the threat model
/// records that as residual risk R18.
pub trait StoreKeyProvider {
    /// Returns the 32-byte store key.
    fn store_key(&self) -> Result<Zeroizing<[u8; STORE_KEY_BYTES]>, StoreKeyError>;
}

/// A store key the host already holds.
pub struct StaticStoreKey(Zeroizing<[u8; STORE_KEY_BYTES]>);

impl StaticStoreKey {
    /// Wraps a key.
    pub fn new(key: [u8; STORE_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(key))
    }

    /// Wraps a key given as a slice, refusing anything but 32 bytes or an
    /// all-zero key (the value an unset buffer has, never a real key).
    pub fn from_slice(key: &[u8]) -> Result<Self, StoreKeyError> {
        let key: [u8; STORE_KEY_BYTES] = key.try_into().map_err(|_| {
            StoreKeyError::Malformed(format!(
                "a store key is {STORE_KEY_BYTES} bytes, got {}",
                key.len()
            ))
        })?;
        let key = Zeroizing::new(key);
        refuse_all_zero(&key)?;
        Ok(Self(key))
    }
}

impl std::fmt::Debug for StaticStoreKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StaticStoreKey(<redacted>)")
    }
}

impl StoreKeyProvider for StaticStoreKey {
    fn store_key(&self) -> Result<Zeroizing<[u8; STORE_KEY_BYTES]>, StoreKeyError> {
        refuse_all_zero(&self.0)?;
        Ok(self.0.clone())
    }
}

/// A store key read from an environment variable, as 64 hex digits or as
/// standard base64 (padded or not).
///
/// This is how a service manager usually hands a process a secret. The
/// variable is read once, when the store opens. The SDK cannot scrub the
/// process environment, so the host should unset the variable after the
/// store is open if nothing else needs it.
#[derive(Debug, Clone)]
pub struct EnvStoreKey {
    variable: String,
}

impl EnvStoreKey {
    /// The variable [`EnvStoreKey::default`] reads.
    pub const DEFAULT_VARIABLE: &'static str = "OFFLINE_PROTOCOL_STORE_KEY";

    /// Reads the key from `variable`.
    pub fn new(variable: impl Into<String>) -> Self {
        Self {
            variable: variable.into(),
        }
    }
}

impl Default for EnvStoreKey {
    fn default() -> Self {
        Self::new(Self::DEFAULT_VARIABLE)
    }
}

impl StoreKeyProvider for EnvStoreKey {
    fn store_key(&self) -> Result<Zeroizing<[u8; STORE_KEY_BYTES]>, StoreKeyError> {
        key_from_variable(&self.variable, std::env::var(&self.variable))
    }
}

/// Turns what the environment holds for `variable` into a key. Separate from
/// the read so tests exercise it without mutating the process environment,
/// which is unsound to do while other test threads read it.
fn key_from_variable(
    variable: &str,
    value: Result<String, std::env::VarError>,
) -> Result<Zeroizing<[u8; STORE_KEY_BYTES]>, StoreKeyError> {
    let raw = match value {
        Ok(raw) => Zeroizing::new(raw),
        Err(std::env::VarError::NotPresent) => {
            return Err(StoreKeyError::Missing(format!(
                "the environment variable {variable} is not set"
            )))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(StoreKeyError::Malformed(format!(
                "the environment variable {variable} is not valid text"
            )))
        }
    };
    let decoded = decode_key_text(raw.trim()).ok_or_else(|| {
        StoreKeyError::Malformed(format!(
            "{variable} must be {STORE_KEY_BYTES} bytes as 64 hex digits or as base64"
        ))
    })?;
    StaticStoreKey::from_slice(&decoded)?.store_key()
}

fn decode_key_text(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    if text.len() == 2 * STORE_KEY_BYTES && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return hex::decode(text).ok().map(Zeroizing::new);
    }
    let engines = [
        base64::engine::general_purpose::STANDARD,
        base64::engine::general_purpose::STANDARD_NO_PAD,
    ];
    engines
        .iter()
        .find_map(|engine| engine.decode(text).ok())
        .filter(|bytes| bytes.len() == STORE_KEY_BYTES)
        .map(Zeroizing::new)
}

fn refuse_all_zero(key: &[u8; STORE_KEY_BYTES]) -> Result<(), StoreKeyError> {
    if key.iter().all(|&b| b == 0) {
        return Err(StoreKeyError::Malformed(
            "an all-zero store key is an unset buffer, not a key".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the provider does with `value` found in the environment, without
    /// touching the environment: `set_var` races every other test thread's
    /// `getenv`.
    fn from_env(
        variable: &str,
        value: Option<&str>,
    ) -> Result<Zeroizing<[u8; STORE_KEY_BYTES]>, StoreKeyError> {
        key_from_variable(
            variable,
            value
                .map(str::to_string)
                .ok_or(std::env::VarError::NotPresent),
        )
    }

    #[test]
    fn hex_and_base64_both_decode_to_the_same_key() {
        let key: Vec<u8> = (1u8..=32).collect();
        let hex_text = hex::encode(&key);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&key);
        let b64_no_pad = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&key);

        for (variable, text) in [
            ("OP_TEST_STORE_KEY_HEX", hex_text.clone()),
            ("OP_TEST_STORE_KEY_HEX_UPPER", hex_text.to_uppercase()),
            ("OP_TEST_STORE_KEY_B64", b64),
            ("OP_TEST_STORE_KEY_B64_NOPAD", b64_no_pad),
            ("OP_TEST_STORE_KEY_PADDED", format!("  {hex_text}\n")),
        ] {
            let got =
                from_env(variable, Some(&text)).unwrap_or_else(|err| panic!("{variable}: {err}"));
            assert_eq!(got.as_slice(), key.as_slice(), "{variable}");
        }
    }

    #[test]
    fn an_absent_short_long_or_zero_key_is_refused() {
        assert!(matches!(
            from_env("OP_TEST_STORE_KEY_ABSENT", None),
            Err(StoreKeyError::Missing(_))
        ));
        for (variable, text) in [
            ("OP_TEST_STORE_KEY_SHORT", hex::encode([7u8; 31])),
            ("OP_TEST_STORE_KEY_LONG", hex::encode([7u8; 33])),
            ("OP_TEST_STORE_KEY_ZERO", hex::encode([0u8; 32])),
            ("OP_TEST_STORE_KEY_EMPTY", String::new()),
            (
                "OP_TEST_STORE_KEY_WORDS",
                "correct horse battery staple".to_string(),
            ),
        ] {
            assert!(
                matches!(
                    from_env(variable, Some(&text)),
                    Err(StoreKeyError::Malformed(_))
                ),
                "{variable} accepted"
            );
        }
    }

    /// The one read of the real environment: a variable nobody sets.
    #[test]
    fn an_unset_variable_is_missing() {
        assert!(matches!(
            EnvStoreKey::new("OFFLINE_PROTOCOL_TEST_STORE_KEY_NEVER_SET").store_key(),
            Err(StoreKeyError::Missing(_))
        ));
    }

    #[test]
    fn a_static_key_is_length_checked_and_redacted() {
        assert!(StaticStoreKey::from_slice(&[1u8; 31]).is_err());
        assert!(StaticStoreKey::from_slice(&[0u8; 32]).is_err());
        assert!(StaticStoreKey::new([0u8; 32]).store_key().is_err());
        let key = StaticStoreKey::from_slice(&[0xabu8; 32]).expect("key");
        assert_eq!(format!("{key:?}"), "StaticStoreKey(<redacted>)");
        assert_eq!(*key.store_key().expect("key"), [0xabu8; 32]);
    }
}
