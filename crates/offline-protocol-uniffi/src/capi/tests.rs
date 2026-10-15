//! Tests of the dispatcher, through its safe half. The `extern "C"`
//! functions are tested beside them.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::codec::{Codec, Fields};
use super::{call_static, CallError, Engine, TELEMETRY_OS};
use crate::{MessagePriority, ProtocolConfig, TransportType};

/// The smallest configuration the definition accepts: every field without
/// a default, and nothing else. One transport is on because the engine
/// refuses a configuration with none; the internet slot opens no socket of
/// its own, since the crates are I/O-free and the host drains the queue.
pub(super) fn minimal_config() -> Value {
    json!({
        "app_id": "com.example.capi",
        "profile": "capi-test",
        "ble_enabled": false,
        "wifi_direct_enabled": false,
        "internet_enabled": true,
        "reticulum_enabled": false,
        "nostr_enabled": false,
        "prefer_online": false,
        "initial_ttl": 8,
        "encryption_enabled": true,
        "auto_key_exchange": true,
        "store_pending": true,
        "max_pending_per_peer": 64,
        "max_pending_global": 4096,
        "pending_ttl_ms": 60000,
        "overflow_policy": "DropOldest"
    })
}

/// A directory removed when dropped.
pub(super) struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("op-capi-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn params_error(result: Result<String, CallError>) -> String {
    match result {
        Err(CallError::InvalidParams(m)) => m,
        other => panic!("expected InvalidParams, got {other:?}"),
    }
}

#[test]
fn a_minimal_config_takes_the_definitions_defaults() {
    let config = ProtocolConfig::decode(&minimal_config(), "config").expect("decodes");
    // `max_group_members = 256`, `data_enabled = true`, `binary_wire_enabled
    // = true` and `mesh_relay = null` in the definition.
    assert_eq!(config.max_group_members, 256);
    assert!(config.data_enabled);
    assert!(config.binary_wire_enabled);
    assert!(config.mesh_relay.is_none());
}

#[test]
fn a_dictionary_missing_a_required_field_names_it() {
    let mut config = minimal_config();
    config.as_object_mut().map(|o| o.remove("profile"));
    let err = ProtocolConfig::decode(&config, "config").expect_err("profile is required");
    assert!(
        matches!(&err, CallError::InvalidParams(m) if m == "config.profile: is required"),
        "{err:?}"
    );
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    let mut config = minimal_config();
    config["profil"] = json!("typo");
    let err = ProtocolConfig::decode(&config, "config").expect_err("a misspelled field");
    assert!(
        format!("{err:?}").contains("has no field \\\"profil\\\""),
        "{err:?}"
    );
}

#[test]
fn enums_cross_by_the_definitions_spelling() {
    // The wrapper's serde spelling is `wifiDirect`; the definition's, and so
    // this boundary's, is `WiFiDirect`.
    assert_eq!(TransportType::WiFiDirect.encode(), json!("WiFiDirect"));
    assert!(matches!(
        TransportType::decode(&json!("WiFiDirect"), "t"),
        Ok(TransportType::WiFiDirect)
    ));
    let err = MessagePriority::decode(&json!("medium"), "p").expect_err("case matters");
    assert!(
        format!("{err:?}").contains("Low, Medium, High, Critical"),
        "{err:?}"
    );
}

#[test]
fn integers_are_held_to_their_declared_width() {
    assert!(matches!(u8::decode(&json!(255), "x"), Ok(255)));
    assert!(u8::decode(&json!(256), "x").is_err());
    assert!(u8::decode(&json!(-1), "x").is_err());
    assert!(u8::decode(&json!(1.5), "x").is_err());
    assert!(u8::decode(&json!(true), "x").is_err());
    assert!(matches!(i16::decode(&json!(-32768), "x"), Ok(-32768)));
    assert!(i16::decode(&json!(32768), "x").is_err());
    assert!(matches!(u64::decode(&json!(u64::MAX), "x"), Ok(u64::MAX)));
    assert!(f32::decode(&json!(1e300), "x").is_err());
}

#[test]
fn bytes_are_padded_standard_base64() {
    let bytes = vec![0u8, 255, 1, 2];
    assert_eq!(bytes.encode(), json!("AP8BAg=="));
    assert!(matches!(Vec::<u8>::decode(&json!("AP8BAg=="), "b"), Ok(b) if b == bytes));
    assert!(
        Vec::<u8>::decode(&json!("AP8BAg"), "b").is_err(),
        "unpadded"
    );
    assert!(
        Vec::<u8>::decode(&json!([0, 255]), "b").is_err(),
        "an array"
    );
}

#[test]
fn an_omitted_optional_parameter_is_null_and_an_extra_one_is_refused() {
    let mut args = Fields::params(Some(json!({"a": 1, "zz": 2})), "m").expect("an object");
    let a: Option<u32> = args.take("a").expect("present");
    let b: Option<u32> = args.take("b").expect("absent is null");
    assert_eq!((a, b), (Some(1), None));
    let err = args.finish().expect_err("zz was never taken");
    assert!(format!("{err:?}").contains("\\\"zz\\\""), "{err:?}");
}

#[test]
fn the_error_position_is_the_definitions_zero_based_one() {
    // Pinned against the local API's table, where MlsNotInitialized is -32008.
    let err = CallError::Engine(crate::ProtocolError::MlsNotInitialized);
    assert_eq!(err.engine_variant(), Some(("MlsNotInitialized", 8)));
    let err = CallError::Engine(crate::ProtocolError::NotStarted);
    assert_eq!(err.engine_variant(), Some(("NotStarted", 0)));
}

#[test]
fn an_instance_less_function_needs_no_engine() {
    let address = call_static(
        "derive_address",
        Some(r#"{"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#),
    )
    .expect("32 zero bytes are a key");
    let address: String = serde_json::from_str(&address).expect("a JSON string");
    assert!(address.starts_with("off1"), "{address}");

    let short = call_static("derive_address", Some(r#"{"public_key":"AAAA"}"#));
    assert!(
        matches!(&short, Err(e) if e.engine_variant().is_some()),
        "a 3-byte key is the engine's refusal, not the codec's: {short:?}"
    );
}

#[test]
fn the_engine_answers_through_the_method_table() {
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    assert_eq!(
        engine.call("get_state", None).expect("state"),
        "\"Stopped\""
    );
    assert_eq!(engine.call("local_address", None).expect("address"), "null");

    match engine.call("no_such_method", None) {
        Err(CallError::MethodNotFound(_)) => {}
        other => panic!("{other:?}"),
    }
    match engine.call("services.no_such_method", None) {
        Err(CallError::MethodNotFound(m)) => assert!(m.contains("services."), "{m}"),
        other => panic!("{other:?}"),
    }
    match engine.call("set_event_callback", None) {
        Err(CallError::MethodNotFound(m)) => assert!(m.contains("docs/bridges/c.md"), "{m}"),
        other => panic!("{other:?}"),
    }
    match engine.call("get_state", Some("{not json")) {
        Err(CallError::InvalidArgument(_)) => {}
        other => panic!("{other:?}"),
    }
    assert!(params_error(engine.call("get_state", Some(r#"{"x":1}"#))).contains("\"x\""));
    // Namespace functions are reachable through an engine too, as on the
    // local API.
    assert!(engine
        .call(
            "derive_address",
            Some(r#"{"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#)
        )
        .is_ok());
}

#[test]
fn sending_before_storage_is_the_engines_refusal() {
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    engine.call("start", None).expect("starts");
    let result = engine.call(
        "send_message",
        Some(r#"{"recipient":"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s","content":"hi","priority":"Medium"}"#),
    );
    // Encryption is required by default and no storage is attached. The
    // local API's error table gives EncryptFailed as -32006.
    let variant = result.as_ref().err().and_then(CallError::engine_variant);
    assert_eq!(variant, Some(("EncryptFailed", 6)), "{result:?}");
    engine.call("stop", None).expect("stops");
}

#[test]
fn file_stores_give_the_engine_an_address() {
    let mls = Scratch::new("mls");
    let state = Scratch::new("state");
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    let params = json!({
        "mls_root": mls.0.to_string_lossy(),
        "state_root": state.0.to_string_lossy(),
        "store_key": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
    });
    engine
        .call("initialize_mls_with_file_stores", Some(&params.to_string()))
        .expect("attaches the stores");
    let address: Option<String> =
        serde_json::from_str(&engine.call("local_address", None).expect("answers")).expect("json");
    assert!(address.is_some_and(|a| a.starts_with("off1")));
    engine.call("close_file_stores", None).expect("closes");
}

#[test]
fn the_binding_fills_the_telemetry_platform_and_refuses_one_from_the_caller() {
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    let refused = engine.call(
        "enable_telemetry",
        Some(r#"{"config":{"api_key":"k","app_id":"a","os":"Ios","os_major":17},"app_state":"Active"}"#),
    );
    assert!(params_error(refused).contains("filled by the binding"));

    // Without `os` the call reaches the engine's own validation: a refusal
    // naming a field, or success, but never the codec's "is required".
    let filled = engine.call(
        "enable_telemetry",
        Some(r#"{"config":{"api_key":"","app_id":"a","os_major":17},"app_state":"Active"}"#),
    );
    if let Err(CallError::InvalidParams(m)) = &filled {
        panic!("the binding did not fill os: {m}");
    }
    let _ = engine.call("disable_telemetry", None);
    // The spelling comes from the codec, so it is one the decoder accepts.
    assert_eq!(
        crate::TelemetryOs::decode(&TELEMETRY_OS.encode(), "os").ok(),
        Some(TELEMETRY_OS)
    );
}

/// The `extern "C"` layer hands one engine to whichever host thread calls
/// it, so the engine must be shareable across threads. A field added later
/// that is not would otherwise surface only as a compile error there.
#[test]
fn the_engine_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Engine>();
}

#[test]
fn a_nested_dictionary_takes_its_own_defaults_and_round_trips() {
    let mut config = minimal_config();
    config["mesh_relay"] = json!({ "fanout": 3, "rate_per_sec": 2.5 });
    let decoded = ProtocolConfig::decode(&config, "config").expect("decodes");
    let relay = decoded.mesh_relay.as_ref().expect("present");
    assert_eq!(relay.fanout, Some(3));
    assert_eq!(relay.rate_per_sec, Some(2.5));
    assert!(relay.max_ttl.is_none() && relay.queue_capacity.is_none());

    // Encoding writes every field, defaults included, and decoding that
    // object again gives the same object: the two directions agree.
    let encoded = decoded.encode();
    let again = ProtocolConfig::decode(&encoded, "config").expect("re-decodes");
    assert_eq!(again.encode(), encoded);

    // An unknown key inside the nested object is refused at its own path.
    config["mesh_relay"] = json!({ "fanot": 3 });
    let err = ProtocolConfig::decode(&config, "config").expect_err("a misspelled field");
    assert!(format!("{err:?}").contains("config.mesh_relay"), "{err:?}");
}

#[test]
fn a_sequence_of_dictionaries_encodes_as_an_array() {
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    let stats: Value =
        serde_json::from_str(&engine.call("get_message_stats", None).expect("answers"))
            .expect("json");
    assert!(stats.is_array(), "{stats}");
}

#[test]
fn a_record_parameter_reaches_the_services_object() {
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    engine
        .call(
            "services.register_service",
            Some(r#"{"service_id":"svc.capi","version":"1","capabilities":{"k":"v"}}"#),
        )
        .expect("registers");
    let removed = engine
        .call(
            "services.unregister_service",
            Some(r#"{"service_id":"svc.capi"}"#),
        )
        .expect("unregisters");
    assert_eq!(removed, "true", "the registration landed");

    let not_a_record = engine.call(
        "services.register_service",
        Some(r#"{"service_id":"s","version":"1","capabilities":{"k":1}}"#),
    );
    assert!(params_error(not_a_record).contains("params.capabilities.k"));
}

#[test]
fn the_data_object_is_reached_through_its_prefix() {
    let mls = Scratch::new("data-mls");
    let state = Scratch::new("data-state");
    let engine = Engine::create(&minimal_config().to_string()).expect("creates");
    let params = json!({
        "mls_root": mls.0.to_string_lossy(),
        "state_root": state.0.to_string_lossy(),
        "store_key": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
    });
    engine
        .call("initialize_mls_with_file_stores", Some(&params.to_string()))
        .expect("attaches the stores");

    engine
        .call(
            "data.create_doc",
            Some(r#"{"space_id":"capi","doc_id":"notes"}"#),
        )
        .expect("creates a document");
    let docs: Vec<String> = serde_json::from_str(
        &engine
            .call("data.list_docs", Some(r#"{"space_id":"capi"}"#))
            .expect("lists"),
    )
    .expect("json");
    assert!(docs.iter().any(|d| d == "notes"), "{docs:?}");

    match engine.call("data.no_such_method", None) {
        Err(CallError::MethodNotFound(m)) => assert!(m.contains("data."), "{m}"),
        other => panic!("{other:?}"),
    }
    engine.call("close_file_stores", None).expect("closes");
}
