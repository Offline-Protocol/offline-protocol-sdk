//! The C binding: the local API's method table, called in process.
//!
//! The engine is driven through one JSON entry point rather than one C
//! function per declaration. A method is named as the local API names it
//! (bare for the engine and the namespace, `services.` and `data.` for the
//! two other objects), its parameters are a JSON object keyed by the
//! definition's parameter names, and its result is JSON. See
//! [the bridge contract](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/bridges/c.md).
//!
//! Why this shape: events already cross every boundary as JSON (bridge
//! contract C3), so a C host parses JSON whatever the call surface is, and a
//! JSON surface keeps the C ABI unchanged when the definition grows. The
//! per-declaration dispatch is generated from the definition (`generated.rs`,
//! written by `generator.rs`), so a method added there is callable from C
//! without anyone writing a line here.
//!
//! This module is the safe half. The `extern "C"` functions sit on top of it.

mod codec;
#[rustfmt::skip]
mod generated;
#[cfg(test)]
mod generator;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use serde_json::Value;

pub use codec::CallError;
use codec::{Codec, Fields};

use crate::{DataStore, MeshServices, OfflineProtocol, ProtocolConfig};

/// The `TelemetryConfig.os` this build reports. The binding fills it, and
/// never reads it from the application (bridge contract C12): a batch's
/// platform is the platform the library was built for, not a string an
/// application forwarded.
pub const TELEMETRY_OS: &str = if cfg!(target_os = "ios") {
    "Ios"
} else if cfg!(target_os = "android") {
    "Android"
} else if cfg!(target_os = "linux") {
    "Linux"
} else if cfg!(target_os = "macos") {
    "Macos"
} else if cfg!(target_os = "windows") {
    "Windows"
} else {
    "Other"
};

/// One engine and the two objects built on it.
///
/// `MeshServices` and `DataStore` are constructed on the first `services.`
/// or `data.` call rather than at creation, so an engine that is only ever
/// driven through the bare method names never builds them. Both constructors
/// are fallible in the definition; a construction that fails leaves its slot
/// empty, so the next call tries again rather than repeating a remembered
/// refusal.
pub struct Engine {
    protocol: Arc<OfflineProtocol>,
    services: Mutex<Option<Arc<MeshServices>>>,
    data: Mutex<Option<Arc<DataStore>>>,
}

fn parse_json(text: Option<&str>, what: &str) -> Result<Option<Value>, CallError> {
    text.map(|t| {
        serde_json::from_str(t)
            .map_err(|e| CallError::InvalidArgument(format!("{what} is not JSON: {e}")))
    })
    .transpose()
}

fn render(value: &Value) -> String {
    value.to_string()
}

impl Engine {
    /// Constructs the engine from a `ProtocolConfig` as a JSON object. Fields
    /// the definition gives a default may be omitted.
    pub fn create(config_json: &str) -> Result<Self, CallError> {
        let value = parse_json(Some(config_json), "the configuration")?.unwrap_or(Value::Null);
        let config = ProtocolConfig::decode(&value, "config")?;
        let protocol = OfflineProtocol::new(config)?;
        Ok(Self {
            protocol: Arc::new(protocol),
            services: Mutex::new(None),
            data: Mutex::new(None),
        })
    }

    /// The engine object, for the entry points that take a callback.
    pub fn protocol(&self) -> &OfflineProtocol {
        &self.protocol
    }

    fn services(&self) -> Result<Arc<MeshServices>, CallError> {
        let mut slot = self.services.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = slot.as_ref() {
            return Ok(existing.clone());
        }
        let built = Arc::new(MeshServices::new(self.protocol.clone())?);
        *slot = Some(built.clone());
        Ok(built)
    }

    fn data(&self) -> Result<Arc<DataStore>, CallError> {
        let mut slot = self.data.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = slot.as_ref() {
            return Ok(existing.clone());
        }
        let built = Arc::new(DataStore::new(self.protocol.clone())?);
        *slot = Some(built.clone());
        Ok(built)
    }

    /// Calls `method` with `params_json` (an object, or `None` for no
    /// parameters) and returns the result as JSON text.
    pub fn call(&self, method: &str, params_json: Option<&str>) -> Result<String, CallError> {
        refuse_hand_written(method)?;
        let mut params = parse_json(params_json, "the parameters")?;
        if method == "enable_telemetry" {
            fill_telemetry_platform(&mut params)?;
        }
        let args = Fields::params(params, method)?;
        let value = if let Some(rest) = method.strip_prefix("services.") {
            generated::call_services(self.services()?.as_ref(), rest, args)?
        } else if let Some(rest) = method.strip_prefix("data.") {
            generated::call_data(self.data()?.as_ref(), rest, args)?
        } else if generated::NAMESPACE.contains(&method) {
            generated::call_namespace(method, args)?
        } else {
            generated::call_engine(&self.protocol, method, args)?
        };
        Ok(render(&value))
    }
}

/// Calls one of the definition's instance-less functions (`derive_address`,
/// `parse_invite`, `verify_identity_assertion`), which need no engine.
pub fn call_static(method: &str, params_json: Option<&str>) -> Result<String, CallError> {
    refuse_hand_written(method)?;
    let args = Fields::params(parse_json(params_json, "the parameters")?, method)?;
    generated::call_namespace(method, args).map(|v| render(&v))
}

fn refuse_hand_written(method: &str) -> Result<(), CallError> {
    if generated::HAND_WRITTEN.contains(&method) {
        return Err(CallError::MethodNotFound(format!(
            "{method} takes a callback or an object and is not reachable through call; \
             see docs/bridges/c.md for its entry point"
        )));
    }
    Ok(())
}

/// Puts this build's platform into `params.config.os`, refusing one the
/// caller supplied. `os_major` stays the caller's: the host is the platform
/// here, and Rust has no portable way to read the operating system's major
/// version.
fn fill_telemetry_platform(params: &mut Option<Value>) -> Result<(), CallError> {
    let Some(config) = params
        .as_mut()
        .and_then(Value::as_object_mut)
        .and_then(|p| p.get_mut("config"))
        .and_then(Value::as_object_mut)
    else {
        // Missing or malformed: the decoder names what is wrong.
        return Ok(());
    };
    if config.contains_key("os") {
        return Err(CallError::InvalidParams(
            "params.config.os is filled by the binding from the platform it was built for; \
             omit it"
                .to_owned(),
        ));
    }
    config.insert("os".to_owned(), Value::from(TELEMETRY_OS));
    Ok(())
}
