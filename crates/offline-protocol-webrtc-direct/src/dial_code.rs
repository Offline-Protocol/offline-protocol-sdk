//! The dial code: everything a page needs to dial a listener, as one URL
//! fragment.
//!
//! The format is the table in `docs/spec/stream-framing.md`, section "The dial
//! code", and this module is the listener's half of it: [`DialCode::to_fragment`]
//! writes a code and [`DialCode::parse`] reads one under every rule the
//! table states. The page's `parseDirectCode` is the other implementation.
//! Both replay `tests/data/webrtc-direct-dial-v1.vectors.json`, which
//! `tools/spec-vectors/generate.py` computes from the chapter and not from
//! this module.
//!
//! # Invariants
//!
//! - **The host is an IP literal on the local network.** A private
//!   (RFC 1918, `fc00::/7`) or link-local (`169.254.0.0/16`, `fe80::/10`)
//!   address, never loopback and never a name. A name would be resolved by
//!   the browser and could point the dial anywhere; a public address would
//!   send a page's traffic off the network it is on.
//! - **No value is ever percent-decoded.** Every value comes from an alphabet
//!   that needs no encoding, so a `%` anywhere refuses the code. A decoder
//!   would make two spellings of one code, and a second spelling is how a
//!   check on one is skipped on the other.
//! - **Keys are looked up by name.** Order does not matter, an unknown key is
//!   ignored, and a repeated or missing key refuses the code, so a code
//!   cannot carry two hosts of which each reader picks a different one.

use core::fmt;
use core::str::FromStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use offline_protocol_core::Address;

/// The only version this crate writes or reads.
pub const DIAL_CODE_VERSION: &str = "1";

/// The longest fragment a dialler reads, in characters, `#` excluded.
pub const MAX_FRAGMENT_LEN: usize = 512;

/// The length of the certificate hash in the code: SHA-256 in base64url
/// with no padding.
pub const CERTHASH_TEXT_LEN: usize = 43;

/// Bounds on the ICE username fragment, in characters.
pub const UFRAG_LEN: core::ops::RangeInclusive<usize> = 4..=32;

/// Bounds on the ICE password, in characters.
pub const PASSWORD_LEN: core::ops::RangeInclusive<usize> = 22..=64;

/// Every key the format defines, in the order a listener writes them.
const KEYS: [&str; 7] = ["op-direct", "h", "p", "c", "u", "w", "a"];

/// What a page needs to dial a listener: where it is, which certificate it
/// will present, the ICE credentials it checks every request under, and the
/// address its preamble must prove.
#[derive(Clone, PartialEq, Eq)]
pub struct DialCode {
    /// The listener's IP address on the local network.
    pub host: IpAddr,
    /// The listener's UDP port, never 0.
    pub port: u16,
    /// SHA-256 of the listener's DER certificate.
    pub certhash: [u8; 32],
    /// The listener's ICE username fragment.
    pub ufrag: String,
    /// The listener's ICE password.
    pub password: String,
    /// The listener's canonical address (`off1…`), the claim the preamble
    /// must prove.
    pub address: String,
}

// The password admits a datagram to a listener, so it stays out of logs.
impl fmt::Debug for DialCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialCode")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("certhash", &URL_SAFE_NO_PAD.encode(self.certhash))
            .field("ufrag", &self.ufrag)
            .field("password", &"<redacted>")
            .field("address", &self.address)
            .finish()
    }
}

/// Why a code was refused. Each names the rule it broke, so a dialler can
/// tell the person which part of a scanned code is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DialCodeError {
    /// The fragment is over [`MAX_FRAGMENT_LEN`] characters.
    #[error("the code is over {MAX_FRAGMENT_LEN} characters")]
    TooLong,
    /// The fragment contains `%`. No value is percent-encoded, so none is
    /// ever decoded.
    #[error("the code contains a percent-encoded value")]
    PercentEncoded,
    /// A part of the fragment is not `key=value`.
    #[error("the code has a part that is not key=value")]
    Malformed,
    /// A key the format requires is absent.
    #[error("the code has no `{0}`")]
    MissingKey(&'static str),
    /// A key the format defines appears twice.
    #[error("the code names `{0}` twice")]
    RepeatedKey(&'static str),
    /// `op-direct` is not `1`.
    #[error("the code's version is not one this dialler supports")]
    UnsupportedVersion,
    /// `h` is not an IP literal in the text form the table names.
    #[error("the code's host is not an IP address literal")]
    HostNotLiteral,
    /// `h` is a loopback address.
    #[error("the code's host is a loopback address")]
    HostLoopback,
    /// `h` is neither private nor link-local.
    #[error("the code's host is not a private or link-local address")]
    HostNotLocal,
    /// `p` is not a decimal port from 1 to 65535.
    #[error("the code's port is not from 1 to 65535")]
    Port,
    /// `c` is not a 43-character base64url SHA-256.
    #[error("the code's certificate hash is not a 43-character base64url SHA-256")]
    CertHash,
    /// `u` is not 4 to 32 characters of `A-Z a-z 0-9`.
    #[error("the code's ICE username is not 4 to 32 letters and digits")]
    Ufrag,
    /// `w` is not 22 to 64 characters of `A-Z a-z 0-9`.
    #[error("the code's ICE password is not 22 to 64 letters and digits")]
    Password,
    /// `a` is not a canonical address.
    #[error("the code's address is not a canonical address")]
    Address,
}

impl DialCode {
    /// The fragment, without the leading `#`: `op-direct=1&h=…&p=…&c=…&u=…&w=…&a=…`.
    ///
    /// `op-direct` is written first so a person or a scanner can recognise
    /// the code. An application places it after the page it wants opened.
    pub fn to_fragment(&self) -> String {
        format!(
            "op-direct={DIAL_CODE_VERSION}&h={}&p={}&c={}&u={}&w={}&a={}",
            self.host,
            self.port,
            URL_SAFE_NO_PAD.encode(self.certhash),
            self.ufrag,
            self.password,
            self.address,
        )
    }

    /// Reads a code from a whole URL, a `#fragment` or a bare fragment.
    ///
    /// Everything after the first `#` is the fragment when there is one,
    /// and the whole input otherwise.
    ///
    /// # Errors
    ///
    /// The [`DialCodeError`] naming the first rule the code breaks.
    pub fn parse(input: &str) -> Result<DialCode, DialCodeError> {
        let fragment = match input.split_once('#') {
            Some((_, fragment)) => fragment,
            None => input,
        };
        if fragment.chars().count() > MAX_FRAGMENT_LEN {
            return Err(DialCodeError::TooLong);
        }
        if fragment.contains('%') {
            return Err(DialCodeError::PercentEncoded);
        }

        let mut values: [Option<&str>; KEYS.len()] = [None; KEYS.len()];
        for part in fragment.split('&') {
            let (key, value) = part.split_once('=').ok_or(DialCodeError::Malformed)?;
            let Some(slot) = KEYS.iter().position(|known| *known == key) else {
                continue;
            };
            if values[slot].replace(value).is_some() {
                return Err(DialCodeError::RepeatedKey(KEYS[slot]));
            }
        }
        let value = |slot: usize| values[slot].ok_or(DialCodeError::MissingKey(KEYS[slot]));
        let (version, host, port, certhash, ufrag, password, address) = (
            value(0)?,
            value(1)?,
            value(2)?,
            value(3)?,
            value(4)?,
            value(5)?,
            value(6)?,
        );

        if version != DIAL_CODE_VERSION {
            return Err(DialCodeError::UnsupportedVersion);
        }
        Ok(DialCode {
            host: parse_host(host)?,
            port: parse_port(port)?,
            certhash: parse_certhash(certhash)?,
            ufrag: parse_ice(ufrag, UFRAG_LEN).ok_or(DialCodeError::Ufrag)?,
            password: parse_ice(password, PASSWORD_LEN).ok_or(DialCodeError::Password)?,
            address: Address::from_str(address)
                .map_err(|_| DialCodeError::Address)?
                .to_string(),
        })
    }
}

/// Whether a dial code may name `ip`: private or link-local, never
/// loopback. The listener applies the same rule to the address it is bound
/// to, so it never writes a code a dialler refuses.
///
/// # Errors
///
/// [`DialCodeError::HostLoopback`] or [`DialCodeError::HostNotLocal`].
pub fn check_host(ip: IpAddr) -> Result<(), DialCodeError> {
    if ip.is_loopback() {
        return Err(DialCodeError::HostLoopback);
    }
    let local = match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        // fc00::/7 (unique local) and fe80::/10 (link-local unicast).
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    };
    if local {
        Ok(())
    } else {
        Err(DialCodeError::HostNotLocal)
    }
}

/// An IPv4 dotted-decimal or IPv6 RFC 5952 literal. IPv6 must be in its one
/// canonical spelling (lowercase, longest zero run compressed), which is
/// what the standard library writes, and carries no brackets and no zone.
fn parse_host(text: &str) -> Result<IpAddr, DialCodeError> {
    let ip = if text.contains(':') {
        let v6 = Ipv6Addr::from_str(text).map_err(|_| DialCodeError::HostNotLiteral)?;
        if v6.to_string() != text {
            return Err(DialCodeError::HostNotLiteral);
        }
        IpAddr::V6(v6)
    } else {
        IpAddr::V4(Ipv4Addr::from_str(text).map_err(|_| DialCodeError::HostNotLiteral)?)
    };
    check_host(ip)?;
    Ok(ip)
}

/// Decimal digits only: `u16::from_str` also takes a leading `+`, which is
/// not in the alphabet. No leading zero, so one port has one spelling (and
/// `0` itself is no port).
fn parse_port(text: &str) -> Result<u16, DialCodeError> {
    if text.is_empty()
        || text.len() > 5
        || text.starts_with('0')
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(DialCodeError::Port);
    }
    match text.parse::<u16>() {
        Ok(0) | Err(_) => Err(DialCodeError::Port),
        Ok(port) => Ok(port),
    }
}

fn parse_certhash(text: &str) -> Result<[u8; 32], DialCodeError> {
    if text.len() != CERTHASH_TEXT_LEN
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(DialCodeError::CertHash);
    }
    // The engine refuses non-zero trailing bits, so one hash has one spelling.
    let bytes = URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| DialCodeError::CertHash)?;
    bytes.try_into().map_err(|_| DialCodeError::CertHash)
}

fn parse_ice(text: &str, bounds: core::ops::RangeInclusive<usize>) -> Option<String> {
    (bounds.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn address() -> String {
        Address::from_hash_bytes([7; Address::HASH_LEN]).to_string()
    }

    fn code() -> DialCode {
        DialCode {
            host: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 23)),
            port: 50123,
            certhash: [0xab; 32],
            ufrag: "AbCdEfGhIjKlMnOp".to_string(),
            password: "0123456789abcdefABCDEFGHIJKLmnop".to_string(),
            address: address(),
        }
    }

    /// The fragment with one key's value replaced, or the key removed.
    fn with(key: &str, value: Option<&str>) -> String {
        code()
            .to_fragment()
            .split('&')
            .filter_map(|part| {
                let (k, _) = part.split_once('=').unwrap();
                if k != key {
                    return Some(part.to_string());
                }
                value.map(|v| format!("{k}={v}"))
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    #[test]
    fn a_code_round_trips_through_its_fragment() {
        let fragment = code().to_fragment();
        assert!(fragment.starts_with("op-direct=1&h="), "{fragment}");
        assert_eq!(DialCode::parse(&fragment).unwrap(), code());
        assert_eq!(DialCode::parse(&format!("#{fragment}")).unwrap(), code());
        let url = format!("https://example.org/app/?x=1#{fragment}");
        assert_eq!(DialCode::parse(&url).unwrap(), code());
    }

    #[test]
    fn an_ipv6_code_round_trips() {
        for host in ["fe80::1c2b:3aff:fe4d:5e6f", "fd12:3456:789a::1"] {
            let mut c = code();
            c.host = host.parse().unwrap();
            let fragment = c.to_fragment();
            assert!(fragment.contains(&format!("h={host}&")), "{fragment}");
            assert_eq!(DialCode::parse(&fragment).unwrap(), c);
        }
    }

    #[test]
    fn link_local_and_every_private_v4_range_are_accepted() {
        for host in [
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.0.1",
            "169.254.7.7",
        ] {
            assert!(DialCode::parse(&with("h", Some(host))).is_ok(), "{host}");
        }
    }

    #[test]
    fn keys_are_read_by_name_in_any_order() {
        let mut parts: Vec<String> = code()
            .to_fragment()
            .split('&')
            .map(str::to_string)
            .collect();
        parts.reverse();
        assert_eq!(DialCode::parse(&parts.join("&")).unwrap(), code());
    }

    #[test]
    fn an_unknown_key_is_ignored() {
        let fragment = format!("{}&future=whatever&x=", code().to_fragment());
        assert_eq!(DialCode::parse(&fragment).unwrap(), code());
    }

    #[test]
    fn every_missing_key_is_refused() {
        for key in KEYS {
            assert_eq!(
                DialCode::parse(&with(key, None)),
                Err(DialCodeError::MissingKey(key)),
                "{key}"
            );
        }
    }

    #[test]
    fn a_repeated_key_is_refused_even_with_the_same_value() {
        for key in KEYS {
            let fragment = code().to_fragment();
            let part = fragment
                .split('&')
                .find(|p| p.starts_with(&format!("{key}=")))
                .unwrap()
                .to_string();
            assert_eq!(
                DialCode::parse(&format!("{fragment}&{part}")),
                Err(DialCodeError::RepeatedKey(key)),
                "{key}"
            );
        }
    }

    #[test]
    fn another_version_is_refused() {
        for version in ["2", "0", "", "01"] {
            assert_eq!(
                DialCode::parse(&with("op-direct", Some(version))),
                Err(DialCodeError::UnsupportedVersion),
                "{version:?}"
            );
        }
    }

    #[test]
    fn a_public_host_is_refused() {
        for host in [
            "8.8.8.8",
            "172.32.0.1",
            "100.64.0.1",
            "2001:db8::1",
            "::ffff:192.168.1.1",
        ] {
            assert_eq!(
                DialCode::parse(&with("h", Some(host))),
                Err(DialCodeError::HostNotLocal),
                "{host}"
            );
        }
    }

    #[test]
    fn a_loopback_host_is_refused() {
        for host in ["127.0.0.1", "127.8.9.10", "::1"] {
            assert_eq!(
                DialCode::parse(&with("h", Some(host))),
                Err(DialCodeError::HostLoopback),
                "{host}"
            );
        }
    }

    #[test]
    fn a_name_or_a_non_canonical_literal_is_refused() {
        for host in [
            "printer.local",
            "localhost",
            "[fe80::1]",
            "FE80::1",
            "fe80:0:0:0:0:0:0:1",
            "192.168.001.1",
            "192.168.1",
            "",
        ] {
            assert_eq!(
                DialCode::parse(&with("h", Some(host))),
                Err(DialCodeError::HostNotLiteral),
                "{host}"
            );
        }
    }

    #[test]
    fn a_port_outside_one_to_65535_is_refused() {
        for port in [
            "0", "65536", "", "+80", "-1", "8o", "123456", "080", "00080",
        ] {
            assert_eq!(
                DialCode::parse(&with("p", Some(port))),
                Err(DialCodeError::Port),
                "{port:?}"
            );
        }
        assert_eq!(DialCode::parse(&with("p", Some("1"))).unwrap().port, 1);
        assert_eq!(
            DialCode::parse(&with("p", Some("65535"))).unwrap().port,
            65535
        );
    }

    #[test]
    fn a_hash_of_the_wrong_length_or_alphabet_is_refused() {
        let good = URL_SAFE_NO_PAD.encode([0xab; 32]);
        let short = &good[..42];
        let long = format!("{good}A");
        let padded = format!("{short}=");
        let standard = format!("{}+", &good[..42]);
        // 43 characters whose last one leaves non-zero trailing bits.
        let trailing = format!("{}B", &good[..42]);
        for hash in [short, &long, &padded, &standard, &trailing] {
            assert_eq!(
                DialCode::parse(&with("c", Some(hash))),
                Err(DialCodeError::CertHash),
                "{hash}"
            );
        }
    }

    #[test]
    fn a_ufrag_outside_its_bounds_or_alphabet_is_refused() {
        for ufrag in ["abc", &"a".repeat(33), "abcd-efg", "abcd_efg", "abcdéfg"] {
            assert_eq!(
                DialCode::parse(&with("u", Some(ufrag))),
                Err(DialCodeError::Ufrag),
                "{ufrag}"
            );
        }
        for ufrag in ["abcd", &"Z9".repeat(16)] {
            assert_eq!(
                DialCode::parse(&with("u", Some(ufrag))).unwrap().ufrag,
                ufrag
            );
        }
    }

    #[test]
    fn a_password_outside_its_bounds_or_alphabet_is_refused() {
        let at_floor = "p".repeat(22);
        let at_ceiling = "p".repeat(64);
        for password in [
            "p".repeat(21),
            "p".repeat(65),
            format!("{}+", "p".repeat(22)),
            format!("{}/", "p".repeat(22)),
        ] {
            assert_eq!(
                DialCode::parse(&with("w", Some(&password))),
                Err(DialCodeError::Password),
                "{password}"
            );
        }
        for password in [at_floor, at_ceiling] {
            assert_eq!(
                DialCode::parse(&with("w", Some(&password)))
                    .unwrap()
                    .password,
                password
            );
        }
    }

    #[test]
    fn a_non_canonical_address_is_refused() {
        let canonical = address();
        for address in ["", "off1", &canonical.to_uppercase(), &canonical[..43]] {
            assert_eq!(
                DialCode::parse(&with("a", Some(address))),
                Err(DialCodeError::Address),
                "{address}"
            );
        }
    }

    #[test]
    fn a_percent_anywhere_is_refused_and_never_decoded() {
        // %41 is "A": a decoder would read a valid ufrag here.
        assert_eq!(
            DialCode::parse(&with("u", Some("%41bcdefgh"))),
            Err(DialCodeError::PercentEncoded)
        );
        // A zone is refused by the same rule.
        assert_eq!(
            DialCode::parse(&with("h", Some("fe80::1%en0"))),
            Err(DialCodeError::PercentEncoded)
        );
        let fragment = format!("{}&unknown=%20", code().to_fragment());
        assert_eq!(
            DialCode::parse(&fragment),
            Err(DialCodeError::PercentEncoded)
        );
    }

    #[test]
    fn a_fragment_over_512_characters_is_refused() {
        let base = code().to_fragment();
        let pad = |n: usize| format!("{base}&z={}", "y".repeat(n - base.len() - 3));
        assert_eq!(pad(512).len(), 512);
        assert!(DialCode::parse(&pad(512)).is_ok());
        assert_eq!(DialCode::parse(&pad(513)), Err(DialCodeError::TooLong));
        // The limit is on the fragment, not on the page URL before it.
        let url = format!("https://example.org/{}#{}", "q".repeat(600), pad(512));
        assert!(DialCode::parse(&url).is_ok());
    }

    #[test]
    fn a_part_that_is_not_key_value_is_refused() {
        let fragment = format!("{}&loose", code().to_fragment());
        assert_eq!(DialCode::parse(&fragment), Err(DialCodeError::Malformed));
    }

    #[test]
    fn the_debug_form_never_shows_the_password() {
        let shown = format!("{:?}", code());
        assert!(!shown.contains(&code().password), "{shown}");
    }
}
