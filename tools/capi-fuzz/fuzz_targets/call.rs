//! What a C host hands `offline_protocol_call`: a method name and JSON
//! parameters, both arbitrary. The input is the name, a newline, and the
//! parameters; without a newline it is a name with no parameters.
//!
//! The property is the boundary's: every answer is a status. A panic is
//! a crash here, as it would be in a host built with `panic = "abort"`
//! (the `minisize` profile), where `catch_unwind` cannot turn it into
//! `Internal`. So is `Internal` itself, the status a caught panic becomes,
//! and so is a success whose result is not JSON.
#![no_main]

use libfuzzer_sys::fuzz_target;
use offline_protocol_uniffi::capi::{call_static, CallError, Engine};

/// Every field of `ProtocolConfig` without a default, one transport on.
const CONFIG: &str = r#"{"app_id":"com.example.fuzz","profile":"fuzz",
"ble_enabled":false,"wifi_direct_enabled":true,"internet_enabled":true,
"reticulum_enabled":true,"nostr_enabled":true,"prefer_online":false,
"initial_ttl":8,"encryption_enabled":true,"auto_key_exchange":true,
"store_pending":true,"max_pending_per_peer":64,"max_pending_global":4096,
"pending_ttl_ms":60000,"overflow_policy":"DropOldest"}"#;

fn check(method: &str, answer: Result<String, CallError>) {
    match answer {
        Ok(text) => {
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&text) {
                panic!("{method} answered text that is not JSON ({e}): {text:?}");
            }
        }
        Err(CallError::Internal(m)) => panic!("{method} answered Internal: {m}"),
        Err(_) => {}
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return; // The boundary refuses text that is not UTF-8 before parsing.
    };
    let (method, params) = match text.split_once('\n') {
        Some((m, p)) => (m, Some(p)),
        None => (text, None),
    };
    // The telemetry pipe is the one part of the library that opens a
    // socket, to a host its configuration names: never from a fuzzer.
    if method.contains("telemetry") {
        return;
    }
    check(method, call_static(method, params));
    // A fresh engine per input, so a crash reproduces from its input alone.
    let engine = Engine::create(CONFIG).expect("the fuzz configuration is valid");
    check(method, engine.call(method, params));
});
