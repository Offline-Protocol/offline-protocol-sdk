//! What an application shows on the air, beside its frames: the Bluetooth LE
//! App tag, the Android Wi-Fi Direct group and its service record, and the
//! browser's nearby room ([ADR 0037], "Nothing in the clear").
//!
//! # Invariants
//!
//! - **A configured application names itself with nothing but `K_app`.**
//!   With [`crate::ProtocolConfig::app_publisher_key`] set, every value here
//!   is derived from the application key, which a party without the
//!   publisher key cannot compute, so none of them names the application to
//!   another application's developer. Without a publisher key nothing here is
//!   derived and each carrier keeps the value it derived from the
//!   application id before this existed, byte for byte: every accessor
//!   returns `None` and the bridge falls back to its own derivation.
//! - **One derivation, one label.** Each value is an HKDF or HMAC output
//!   under a label of its own from `offline_protocol_sealed::membership`, so
//!   no identifier shown on the air is also a key or a tag of the membership
//!   exchange.
//! - **The App tag is per instance.** It is a MAC over the Device id the
//!   instance serves, so two phones of one application serve two unrelated
//!   tags, and it groups nothing for an observer. A central of the same
//!   application recomputes it from the Device id it read.
//! - **Derived once, read without the engine.** The value is fixed by the
//!   configuration, so a binding holds a copy and answers a GATT read on a
//!   binder thread without taking the engine's lock.
//!
//! [ADR 0037]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md

use core::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use offline_protocol_sealed::membership::{
    ble_app_tag_input, APP_KEY_LEN, BLE_APP_TAG_LEN, WEB_NEARBY_INFO, WIFI_DIRECT_APP_INFO,
    WIFI_DIRECT_NETWORK_INFO, WIFI_DIRECT_PASSPHRASE_INFO,
};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::membership::derive_app_key;

/// Bytes of the Wi-Fi Direct record value, rendered as hex.
const WIFI_DIRECT_RECORD_BYTES: usize = 4;

/// Bytes of the Wi-Fi Direct group name after its prefix, rendered as hex.
const WIFI_DIRECT_NETWORK_BYTES: usize = 4;

/// Bytes of the Wi-Fi Direct passphrase, rendered as hex: 32 digits, inside
/// WPA2's 8 to 63.
const WIFI_DIRECT_PASSPHRASE_BYTES: usize = 16;

/// The prefix Android requires of a group name (`DIRECT-` and two
/// characters), and the one the application-id derivation uses.
const WIFI_DIRECT_NETWORK_PREFIX: &str = "DIRECT-op-";

/// What a configured application advertises over Android Wi-Fi Direct, in
/// place of the values the bridge derives from the application id.
#[derive(Clone, PartialEq, Eq)]
pub struct WifiDirectIdentifiers {
    /// The value under the `app` key of the service record: eight hex digits.
    pub record_app: String,
    /// The group name: `DIRECT-op-` and eight hex digits.
    pub network_name: String,
    /// The group passphrase: 32 hex digits.
    pub passphrase: String,
}

impl fmt::Debug for WifiDirectIdentifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiDirectIdentifiers")
            .field("record_app", &self.record_app)
            .field("network_name", &self.network_name)
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

/// The identifiers an application shows on the air, from its application
/// key when it has one. See the module documentation.
#[derive(Clone, Default)]
pub struct AppIdentifiers {
    app_key: Option<Zeroizing<[u8; APP_KEY_LEN]>>,
}

impl fmt::Debug for AppIdentifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppIdentifiers")
            .field("keyed", &self.is_keyed())
            .finish()
    }
}

impl AppIdentifiers {
    /// The identifiers for a configuration: keyed by `K_app` when
    /// `publisher_key` is set, derived exactly as the membership exchange
    /// derives it.
    pub fn new(publisher_key: Option<&[u8; 32]>, app_id: &str) -> Self {
        Self::from_app_key(publisher_key.map(|pk| derive_app_key(pk, app_id)))
    }

    pub(crate) fn from_app_key(app_key: Option<[u8; APP_KEY_LEN]>) -> Self {
        Self {
            app_key: app_key.map(Zeroizing::new),
        }
    }

    /// Whether the application has a publisher key, so every accessor below
    /// returns a value.
    pub fn is_keyed(&self) -> bool {
        self.app_key.is_some()
    }

    /// The App tag an instance of this application serves when its Device id
    /// is `device_id`: `HMAC-SHA256(K_app, label || device_id)[..8]`.
    ///
    /// A peripheral serves it over its own address; a central that finds
    /// several instances on one link compares each instance's App tag with
    /// this over that instance's Device id. `None` without a publisher key:
    /// the bridge serves and compares the tag of the application id, as it
    /// did before.
    pub fn ble_app_tag(&self, device_id: &str) -> Option<[u8; BLE_APP_TAG_LEN]> {
        let key = self.app_key.as_ref()?;
        // HMAC takes a key of any length, so this never refuses one.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref()).ok()?;
        mac.update(&ble_app_tag_input(device_id));
        let digest = mac.finalize().into_bytes();
        let mut tag = [0u8; BLE_APP_TAG_LEN];
        tag.copy_from_slice(&digest[..BLE_APP_TAG_LEN]);
        Some(tag)
    }

    /// The Android Wi-Fi Direct group and record values, or `None` without a
    /// publisher key.
    ///
    /// One per application, as the application-id values are: every member
    /// must find and join the same group. They name nothing, but an observer
    /// sees which devices advertise the same record, so they still group the
    /// devices of one application near each other while a group forms.
    pub fn wifi_direct(&self) -> Option<WifiDirectIdentifiers> {
        let key = self.app_key.as_ref()?;
        let record = expand(key.as_ref(), WIFI_DIRECT_APP_INFO);
        let network = expand(key.as_ref(), WIFI_DIRECT_NETWORK_INFO);
        let passphrase = expand(key.as_ref(), WIFI_DIRECT_PASSPHRASE_INFO);
        Some(WifiDirectIdentifiers {
            record_app: hex(&record[..WIFI_DIRECT_RECORD_BYTES]),
            network_name: format!(
                "{WIFI_DIRECT_NETWORK_PREFIX}{}",
                hex(&network[..WIFI_DIRECT_NETWORK_BYTES])
            ),
            passphrase: hex(&passphrase[..WIFI_DIRECT_PASSPHRASE_BYTES]),
        })
    }

    /// The secret a browser's nearby room is derived from in place of the
    /// application id, or `None` without a publisher key.
    pub fn web_nearby_secret(&self) -> Option<Zeroizing<[u8; 32]>> {
        let key = self.app_key.as_ref()?;
        Some(expand(key.as_ref(), WEB_NEARBY_INFO))
    }
}

/// `HKDF-SHA256(ikm = K_app, salt = none, info)`, 32 bytes. Shorter values
/// are its prefix, which is what HKDF-Expand of fewer bytes gives.
fn expand(app_key: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    // 32 bytes is far below HKDF-SHA256's 8160-byte ceiling, so this cannot
    // fail; the zeros are never reached.
    let _ = Hkdf::<Sha256>::new(None, app_key).expand(info, out.as_mut());
    out
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str =
        include_str!("../../../offline-protocol-sealed/tests/data/app-membership-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn field<'a>(v: &'a serde_json::Value, path: &[&str]) -> &'a str {
        path.iter()
            .fold(v, |v, key| &v[*key])
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is a string"))
    }

    fn configured() -> AppIdentifiers {
        let v = vectors();
        let pk: [u8; 32] = (0..32)
            .map(|i| {
                let text = field(&v, &["app_key", "publisher_public_key_hex"]);
                u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex")
            })
            .collect::<Vec<_>>()
            .try_into()
            .expect("32 bytes");
        AppIdentifiers::new(Some(&pk), field(&v, &["app_key", "app_id"]))
    }

    #[test]
    fn without_a_publisher_key_nothing_is_derived() {
        let ids = AppIdentifiers::new(None, "com.example.chat");
        assert!(!ids.is_keyed());
        assert_eq!(ids.ble_app_tag("off1anything"), None);
        assert_eq!(ids.wifi_direct(), None);
        assert!(ids.web_nearby_secret().is_none());
    }

    #[test]
    fn the_ble_app_tag_matches_the_vectors_and_differs_per_instance() {
        let v = vectors();
        let ids = configured();
        assert!(ids.is_keyed());
        for key in ["a", "b"] {
            let tag = ids
                .ble_app_tag(field(&v, &["addresses", key]))
                .expect("keyed");
            assert_eq!(
                hex(&tag),
                field(
                    &v,
                    &["identifiers", "ble_app_tag", &format!("{key}_tag_hex")]
                )
            );
        }
        assert_ne!(
            field(&v, &["identifiers", "ble_app_tag", "a_tag_hex"]),
            field(&v, &["identifiers", "ble_app_tag", "b_tag_hex"])
        );
    }

    #[test]
    fn the_wifi_direct_values_match_the_vectors() {
        let v = vectors();
        let wd = configured().wifi_direct().expect("keyed");
        assert_eq!(
            wd.record_app,
            field(&v, &["identifiers", "wifi_direct", "record_app"])
        );
        assert_eq!(
            wd.network_name,
            field(&v, &["identifiers", "wifi_direct", "network_name"])
        );
        assert_eq!(
            wd.passphrase,
            field(&v, &["identifiers", "wifi_direct", "passphrase"])
        );
        // What Android requires of a group name, and WPA2 of a passphrase.
        assert!(wd.network_name.starts_with("DIRECT-") && wd.network_name.len() <= 32);
        assert!((8..=63).contains(&wd.passphrase.len()));
        assert!(!format!("{wd:?}").contains(&wd.passphrase));
    }

    #[test]
    fn the_nearby_secret_matches_the_vectors() {
        let v = vectors();
        let secret = configured().web_nearby_secret().expect("keyed");
        assert_eq!(
            hex(secret.as_ref()),
            field(&v, &["identifiers", "web_nearby", "secret_hex"])
        );
    }

    /// Two applications that share a publisher key still show different
    /// values: the application id is in `K_app`.
    #[test]
    fn another_application_under_one_publisher_shows_other_values() {
        let pk = [7u8; 32];
        let (a, b) = (
            AppIdentifiers::new(Some(&pk), "com.example.a"),
            AppIdentifiers::new(Some(&pk), "com.example.b"),
        );
        assert_ne!(a.ble_app_tag("off1x"), b.ble_app_tag("off1x"));
        assert_ne!(a.wifi_direct(), b.wifi_direct());
        assert_ne!(
            a.web_nearby_secret().map(|s| *s),
            b.web_nearby_secret().map(|s| *s)
        );
    }
}
