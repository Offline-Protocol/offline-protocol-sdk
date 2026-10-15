//! The one mapping between the interface definition's types and JSON, for
//! the C binding.
//!
//! It is the local API's encoding table ([the chapter], "Encoding") applied
//! in process: strings and booleans as themselves, integers range-checked to
//! their declared width, `bytes` and `sequence<u8>` as padded standard base64,
//! enums by the definition's own spelling, dictionaries as objects whose
//! omitted fields take the definition's defaults, `void` as `null`.
//!
//! The mapping is chosen by the wrapper's Rust type through [`Codec`], so a
//! parameter is decoded as whatever the wrapper method actually takes. The
//! enum and dictionary implementations are generated from the definition into
//! `generated.rs`; this file holds the primitives and the combinators.
//!
//! It is deliberately not serde on the wrapper types. Those carry no
//! `Deserialize`, and the two that derive `Serialize` (`TransportType`,
//! `TransportMetrics`) spell their names in camelCase (`"wifiDirect"`), which
//! is not the definition's spelling. Deriving here would either change what
//! those already emit or put two spellings of one enum on two boundaries.
//!
//! [the chapter]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/spec/local-api.md#encoding

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{Map, Value};

use crate::ProtocolError;

/// Why a call produced no result.
///
/// Non-exhaustive because this crate is published and the type is public:
/// a refusal class added later must not break a Rust host's `match`.
#[derive(Debug)]
#[non_exhaustive]
pub enum CallError {
    /// A required argument was missing or was not what the boundary accepts:
    /// a null pointer, text that is not UTF-8, parameters that are not JSON.
    InvalidArgument(String),
    /// No declaration of that name.
    MethodNotFound(String),
    /// The parameters parsed as JSON and do not fit the declaration.
    InvalidParams(String),
    /// The engine refused, with the definition's own variant.
    Engine(ProtocolError),
    /// The binding itself failed: a panic caught at the boundary.
    Internal(String),
}

impl CallError {
    /// The human-readable reason, the engine's display string for an engine
    /// refusal.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidArgument(m)
            | Self::MethodNotFound(m)
            | Self::InvalidParams(m)
            | Self::Internal(m) => m.clone(),
            Self::Engine(e) => e.to_string(),
        }
    }

    /// For an engine refusal, the variant's name and its zero-based position
    /// in the definition's error enum: the `n` of the local API's
    /// `-32000 - n`, and never the generated bindings' one-based discriminant.
    pub fn engine_variant(&self) -> Option<(&'static str, i32)> {
        match self {
            Self::Engine(e) => Some(super::generated::protocol_error_variant(e)),
            _ => None,
        }
    }
}

impl From<ProtocolError> for CallError {
    fn from(e: ProtocolError) -> Self {
        Self::Engine(e)
    }
}

pub type Decoded<T> = Result<T, CallError>;

fn invalid(at: &str, what: &str) -> CallError {
    CallError::InvalidParams(format!("{at}: {what}"))
}

/// A wrapper type's JSON form, in both directions.
pub trait Codec: Sized {
    /// Decodes `value`, found at `at` (a path for the error message).
    fn decode(value: &Value, at: &str) -> Decoded<Self>;

    /// The JSON form of `self`.
    fn encode(&self) -> Value;

    /// What an omitted key means. Required unless the type is optional.
    fn absent(at: &str) -> Decoded<Self> {
        Err(invalid(at, "is required"))
    }
}

impl Codec for String {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid(at, "expected a string"))
    }
    fn encode(&self) -> Value {
        Value::String(self.clone())
    }
}

impl Codec for bool {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        value
            .as_bool()
            .ok_or_else(|| invalid(at, "expected true or false"))
    }
    fn encode(&self) -> Value {
        Value::Bool(*self)
    }
}

macro_rules! unsigned {
    ($($t:ty => $name:literal),*) => {$(
        impl Codec for $t {
            fn decode(value: &Value, at: &str) -> Decoded<Self> {
                let wide = value.as_u64().ok_or_else(|| {
                    if value.as_i64().is_some() {
                        invalid(at, concat!("is outside ", $name))
                    } else {
                        invalid(at, "expected an integer")
                    }
                })?;
                <$t>::try_from(wide).map_err(|_| invalid(at, concat!("is outside ", $name)))
            }
            fn encode(&self) -> Value {
                Value::from(*self)
            }
        }
    )*};
}

macro_rules! signed {
    ($($t:ty => $name:literal),*) => {$(
        impl Codec for $t {
            fn decode(value: &Value, at: &str) -> Decoded<Self> {
                let wide = value.as_i64().ok_or_else(|| {
                    if value.as_u64().is_some() {
                        invalid(at, concat!("is outside ", $name))
                    } else {
                        invalid(at, "expected an integer")
                    }
                })?;
                <$t>::try_from(wide).map_err(|_| invalid(at, concat!("is outside ", $name)))
            }
            fn encode(&self) -> Value {
                Value::from(*self)
            }
        }
    )*};
}

unsigned!(u8 => "u8", u16 => "u16", u32 => "u32", u64 => "u64");
signed!(i16 => "i16", i32 => "i32", i64 => "i64");

impl Codec for f64 {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        value
            .as_f64()
            .ok_or_else(|| invalid(at, "expected a number"))
    }
    /// A non-finite value has no JSON form and encodes as `null`.
    fn encode(&self) -> Value {
        Value::from(*self)
    }
}

impl Codec for f32 {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        let wide = f64::decode(value, at)?;
        if wide.abs() > f64::from(f32::MAX) {
            return Err(invalid(at, "is outside f32"));
        }
        #[allow(clippy::cast_possible_truncation)]
        Ok(wide as f32)
    }
    fn encode(&self) -> Value {
        Value::from(f64::from(*self))
    }
}

/// `bytes` and `sequence<u8>` alike: the definition uses both for byte
/// strings, and both are a `Vec<u8>` in the wrapper.
impl Codec for Vec<u8> {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        let text = value
            .as_str()
            .ok_or_else(|| invalid(at, "bytes are a base64 string"))?;
        STANDARD
            .decode(text)
            .map_err(|e| invalid(at, &format!("not valid base64 ({e})")))
    }
    fn encode(&self) -> Value {
        Value::String(STANDARD.encode(self))
    }
}

impl<T: Codec> Codec for Option<T> {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        if value.is_null() {
            Ok(None)
        } else {
            T::decode(value, at).map(Some)
        }
    }
    fn encode(&self) -> Value {
        self.as_ref().map_or(Value::Null, Codec::encode)
    }
    fn absent(_at: &str) -> Decoded<Self> {
        Ok(None)
    }
}

/// Decodes an array whose elements are `T`. A `Vec<T>` is implemented per
/// element type rather than generically, because `Vec<u8>` is bytes.
pub fn decode_array<T: Codec>(value: &Value, at: &str) -> Decoded<Vec<T>> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(at, "expected an array"))?;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| T::decode(item, &format!("{at}[{i}]")))
        .collect()
}

pub fn encode_array<T: Codec>(items: &[T]) -> Value {
    Value::Array(items.iter().map(Codec::encode).collect())
}

impl Codec for Vec<String> {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        decode_array(value, at)
    }
    fn encode(&self) -> Value {
        encode_array(self)
    }
}

/// `record<DOMString, string>`.
impl Codec for HashMap<String, String> {
    fn decode(value: &Value, at: &str) -> Decoded<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid(at, "expected an object"))?;
        object
            .iter()
            .map(|(k, v)| Ok((k.clone(), String::decode(v, &format!("{at}.{k}"))?)))
            .collect()
    }
    fn encode(&self) -> Value {
        Value::Object(
            self.iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        )
    }
}

/// The refusal for a value that is not one of an enum's spellings.
pub fn variant_error(at: &str, type_name: &str, variants: &[&str]) -> CallError {
    invalid(
        at,
        &format!("expected one of {} for {type_name}", variants.join(", ")),
    )
}

/// The keys of one JSON object, taken one at a time, so whatever is left at
/// the end is a key the declaration does not have.
pub struct Fields<'a> {
    at: String,
    type_name: &'a str,
    map: Map<String, Value>,
}

impl<'a> Fields<'a> {
    /// `value` must be an object. `type_name` names it in errors.
    pub fn new(value: &Value, at: &str, type_name: &'a str) -> Decoded<Self> {
        let map = value
            .as_object()
            .cloned()
            .ok_or_else(|| invalid(at, &format!("expected an object shaped as {type_name}")))?;
        Ok(Self {
            at: at.to_owned(),
            type_name,
            map,
        })
    }

    /// Parameters, which may be absent altogether (`NULL` from C) and then
    /// mean an empty object.
    pub fn params(value: Option<Value>, method: &'a str) -> Decoded<Self> {
        match value {
            None | Some(Value::Null) => Ok(Self {
                at: "params".to_owned(),
                type_name: method,
                map: Map::new(),
            }),
            Some(v) => Self::new(&v, "params", method),
        }
    }

    fn path(&self, key: &str) -> String {
        format!("{}.{key}", self.at)
    }

    /// The key's value, or what absence means for its type.
    pub fn take<T: Codec>(&mut self, key: &str) -> Decoded<T> {
        let at = self.path(key);
        match self.map.remove(key) {
            Some(v) => T::decode(&v, &at),
            None => T::absent(&at),
        }
    }

    /// The key's value, or `default` when it is absent. `default` is the
    /// definition's own literal, written by the generator; the binding never
    /// substitutes one of its own (C6).
    pub fn take_or<T: Codec>(&mut self, key: &str, default: T) -> Decoded<T> {
        let at = self.path(key);
        match self.map.remove(key) {
            Some(v) => T::decode(&v, &at),
            None => Ok(default),
        }
    }

    /// Refuses whatever key was not taken. A misspelled key is a mistake the
    /// caller would otherwise never hear about.
    pub fn finish(self) -> Decoded<()> {
        match self.map.keys().next() {
            None => Ok(()),
            Some(key) => Err(invalid(
                &self.at,
                &format!("{} has no field {key:?}", self.type_name),
            )),
        }
    }
}

/// Builds a dictionary's JSON object.
pub fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}
