//! Conformance vectors for the dial code.
//!
//! The chapter these pin is `docs/spec/stream-framing.md`, "The dial code".
//! The vectors are computed by `tools/spec-vectors/generate.py` from the
//! table there rather than by running [`DialCode`], because two copies of one
//! mistake agree perfectly: a vector generated from `parse` would pass
//! against any rule this crate happened to apply, including accepting a
//! loopback host. The page's `parseDirectCode` replays the same file, so the
//! two readers of the format cannot drift apart unnoticed.
//!
//! A failure here means the dial code moved. That needs a new version of the
//! code and a new vector file, not an edited expectation: editing the
//! expected value to make a test pass converts a caught break into a shipped
//! one, and every page already deployed still reads the old format.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use offline_protocol_webrtc_direct::dial_code::{
    CERTHASH_TEXT_LEN, DIAL_CODE_VERSION, MAX_FRAGMENT_LEN, PASSWORD_LEN, UFRAG_LEN,
};
use offline_protocol_webrtc_direct::{DialCode, DialCodeError};
use serde_json::Value;

const VECTORS: &str = include_str!("data/webrtc-direct-dial-v1.vectors.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("the vector file is valid JSON")
}

fn cases<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("the vectors carry `{key}` as an array"))
}

fn text<'a>(case: &'a Value, key: &str) -> &'a str {
    case[key]
        .as_str()
        .unwrap_or_else(|| panic!("[{}] `{key}` is a string", case["name"]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The token the vectors name for each refusal, and the key it concerns.
///
/// No catch-all arm: a new refusal in the crate must be given a token here,
/// and in the vectors' `reasons` if a vector exercises it.
fn reason(error: &DialCodeError) -> (&'static str, Option<&'static str>) {
    match error {
        DialCodeError::TooLong => ("too_long", None),
        DialCodeError::PercentEncoded => ("percent_encoded", None),
        DialCodeError::Malformed => ("malformed", None),
        DialCodeError::MissingKey(key) => ("missing_key", Some(key)),
        DialCodeError::RepeatedKey(key) => ("repeated_key", Some(key)),
        DialCodeError::UnsupportedVersion => ("unsupported_version", None),
        DialCodeError::HostNotLiteral => ("host_not_literal", None),
        DialCodeError::HostLoopback => ("host_loopback", None),
        DialCodeError::HostNotLocal => ("host_not_local", None),
        DialCodeError::Port => ("port", None),
        DialCodeError::CertHash => ("certhash", None),
        DialCodeError::Ufrag => ("ufrag", None),
        DialCodeError::Password => ("password", None),
        DialCodeError::Address => ("address", None),
    }
}

/// Every accepted code reads exactly the values the vectors pin, the
/// listener writes exactly the pinned fragment for them, and that fragment
/// reads back to the same code.
#[test]
fn every_accepted_code_reads_exactly_the_pinned_values() {
    let v = vectors();
    for case in cases(&v, "accept") {
        let name = text(case, "name");
        let expect = &case["expect"];
        let code = DialCode::parse(text(case, "input"))
            .unwrap_or_else(|e| panic!("[{name}] refused: {e}"));

        assert_eq!(code.host.to_string(), text(expect, "host"), "[{name}] host");
        assert_eq!(
            Some(u64::from(code.port)),
            expect["port"].as_u64(),
            "[{name}] port"
        );
        assert_eq!(
            hex(&code.certhash),
            text(expect, "certhash_hex"),
            "[{name}] certhash"
        );
        assert_eq!(code.ufrag, text(expect, "ufrag"), "[{name}] ufrag");
        assert_eq!(code.password, text(expect, "password"), "[{name}] password");
        assert_eq!(code.address, text(expect, "address"), "[{name}] address");

        let written = code.to_fragment();
        assert_eq!(
            written,
            text(case, "fragment"),
            "[{name}] the listener's writing"
        );
        assert_eq!(
            DialCode::parse(&written).as_ref(),
            Ok(&code),
            "[{name}] the writing reads back"
        );
    }
}

/// Every refused code is refused, for the rule the vector was built to
/// break.
#[test]
fn every_refused_code_is_refused_for_the_rule_it_breaks() {
    let v = vectors();
    for case in cases(&v, "refuse") {
        let name = text(case, "name");
        let error = match DialCode::parse(text(case, "input")) {
            Ok(code) => panic!("[{name}] accepted: {code:?}"),
            Err(error) => error,
        };
        let (token, key) = reason(&error);
        assert_eq!(token, text(case, "reason"), "[{name}] reason ({error})");
        assert_eq!(key, case["key"].as_str(), "[{name}] key ({error})");
    }
}

/// The bounds the file was generated under are the ones this crate applies,
/// so a limit changed on one side alone fails here rather than in a page.
#[test]
fn the_file_states_this_crates_limits() {
    let v = vectors();
    let bounds = |key: &str| -> (usize, usize) {
        let pair = cases(&v, key);
        let at = |i: usize| usize::try_from(pair[i].as_u64().expect("a bound")).expect("fits");
        (at(0), at(1))
    };
    assert_eq!(text(&v, "version"), DIAL_CODE_VERSION);
    assert_eq!(
        v["max_fragment_len"].as_u64(),
        Some(MAX_FRAGMENT_LEN as u64)
    );
    assert_eq!(
        v["certhash_text_len"].as_u64(),
        Some(CERTHASH_TEXT_LEN as u64)
    );
    assert_eq!(bounds("ufrag_len"), (*UFRAG_LEN.start(), *UFRAG_LEN.end()));
    assert_eq!(
        bounds("password_len"),
        (*PASSWORD_LEN.start(), *PASSWORD_LEN.end())
    );
}

/// The file still carries what the chapter promises, checked before anything
/// iterates it: a loop over an array a bad merge emptied passes by not
/// running.
#[test]
fn the_vectors_cover_every_rule_the_chapter_names() {
    let v = vectors();
    let accept = cases(&v, "accept");
    let refuse = cases(&v, "refuse");
    assert!(accept.len() >= 20, "only {} accepted codes", accept.len());
    assert!(refuse.len() >= 50, "only {} refused codes", refuse.len());

    // Each of the three forms a code is read from.
    let inputs: Vec<&str> = accept.iter().map(|c| text(c, "input")).collect();
    assert!(
        inputs.iter().any(|i| i.starts_with("https://")),
        "a whole URL"
    );
    assert!(inputs.iter().any(|i| i.starts_with('#')), "a #fragment");
    assert!(
        inputs.iter().any(|i| i.starts_with("op-direct=")),
        "a bare fragment"
    );
    // Both sides of the length limit.
    assert!(
        inputs.iter().any(|i| i.len() == MAX_FRAGMENT_LEN),
        "an accepted fragment of exactly the limit"
    );
    assert!(
        refuse
            .iter()
            .any(|c| text(c, "reason") == "too_long"
                && text(c, "input").len() == MAX_FRAGMENT_LEN + 1),
        "a refused fragment of one over the limit"
    );

    // Each key missing, and each key repeated.
    let keys: BTreeSet<&str> = cases(&v, "keys")
        .iter()
        .map(|k| k.as_str().expect("a key"))
        .collect();
    assert_eq!(keys.len(), 7);
    for kind in ["missing_key", "repeated_key"] {
        let covered: BTreeSet<&str> = refuse
            .iter()
            .filter(|c| text(c, "reason") == kind)
            .map(|c| text(c, "key"))
            .collect();
        assert_eq!(covered, keys, "{kind} for every key");
    }

    // Every documented reason is exercised, and every exercised reason is
    // documented.
    let documented: BTreeSet<&str> = v["reasons"]
        .as_object()
        .expect("reasons")
        .keys()
        .map(String::as_str)
        .collect();
    let used: BTreeSet<&str> = refuse.iter().map(|c| text(c, "reason")).collect();
    assert_eq!(documented, used);
}
