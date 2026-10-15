//! Writes `generated.rs` from the interface definition.
//!
//! Compiled into this crate's tests only, where
//! `the_generated_c_dispatch_is_current` regenerates the file in memory and
//! compares it with the checked-in one, so a change to the definition reaches
//! the C binding as a diff and a red test rather than as a method C cannot
//! find. Rewrite the file with:
//!
//! ```text
//! cargo run --manifest-path tools/capi-dispatch-gen/Cargo.toml
//! ```
//!
//! That tool compiles this one file on its own rather than through this crate,
//! because a declaration renamed or removed in the definition leaves the
//! checked-in dispatcher calling the old name, and this crate then does not
//! compile until the file is regenerated.
//!
//! The parser reads the subset of WebIDL the definition uses, the same subset
//! `bindings/python/scripts/generate_local_api_table.py` reads for the local
//! API. A type form or a default it does not know stops the generator with a
//! message naming it, rather than being guessed at: a guess is a parameter
//! decoded wrongly on one boundary and correctly on the others, which nothing
//! downstream would notice.

use std::collections::BTreeSet;
use std::fmt::Write as _;

/// One declaration block of the definition.
#[derive(Debug)]
pub(super) struct Block {
    pub kind: Kind,
    pub name: String,
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Namespace,
    Interface,
    Callback,
    Dictionary,
    Enum,
}

/// One `;`-terminated statement of a block body, attributes split off.
#[derive(Debug)]
pub(super) struct Statement {
    pub attrs: Vec<String>,
    pub text: String,
}

#[derive(Debug)]
pub(super) struct Method {
    pub name: String,
    pub ret: String,
    pub params: Vec<(String, String)>,
    pub throws: bool,
    pub constructor: bool,
}

#[derive(Debug)]
pub(super) struct Field {
    pub name: String,
    pub ty: String,
    pub default: Option<String>,
}

/// Removes `//` comments. The definition has no `/* */` comments and no
/// string literal containing `//`; the generator refuses a `/*` so that the
/// first one is noticed.
fn strip_comments(udl: &str) -> String {
    assert!(
        !udl.contains("/*"),
        "the definition has a block comment; teach strip_comments about it"
    );
    udl.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Splits `[A, B=c]` off the front of `text`.
fn split_attrs(text: &str) -> (Vec<String>, String) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('[') {
        let end = rest.find(']').expect("an unterminated attribute list");
        let attrs = rest[..end]
            .split(',')
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty())
            .collect();
        (attrs, rest[end + 1..].trim().to_owned())
    } else {
        (Vec::new(), text.to_owned())
    }
}

/// Parses every block of the definition, in order.
pub(super) fn parse(udl: &str) -> Vec<Block> {
    let text = strip_comments(udl);
    let mut blocks = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let (attrs, header) = split_attrs(&rest[..open]);
        assert!(
            attrs.iter().all(|a| a == "Error"),
            "unexpected block attributes {attrs:?} on {header}"
        );
        let words: Vec<&str> = header.split_whitespace().collect();
        let (kind, name) = match words.as_slice() {
            ["namespace", n] => (Kind::Namespace, *n),
            ["interface", n] => (Kind::Interface, *n),
            ["callback", "interface", n] => (Kind::Callback, *n),
            ["dictionary", n] => (Kind::Dictionary, *n),
            ["enum", n] => (Kind::Enum, *n),
            other => panic!("an unknown block header {other:?}"),
        };
        let body_and_more = &rest[open + 1..];
        let close = body_and_more
            .find("};")
            .unwrap_or_else(|| panic!("block {name} is not terminated"));
        let body = &body_and_more[..close];
        assert!(!body.contains('{'), "block {name} nests braces");
        let statements = if kind == Kind::Enum {
            body.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Statement {
                    attrs: Vec::new(),
                    text: s.to_owned(),
                })
                .collect()
        } else {
            body.split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let (attrs, text) = split_attrs(s);
                    Statement { attrs, text }
                })
                .collect()
        };
        blocks.push(Block {
            kind,
            name: name.to_owned(),
            statements,
        });
        rest = &body_and_more[close + 2..];
    }
    assert!(
        rest.trim().is_empty(),
        "text after the last block: {:?}",
        rest.trim()
    );
    blocks
}

/// Whitespace removed, so `record<DOMString, string>` has one spelling.
fn norm(ty: &str) -> String {
    ty.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Splits a parameter list on the commas outside angle brackets.
fn split_params(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for ch in text.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    out.push(current);
    out.into_iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// `type name` with the name last.
fn split_typed(text: &str) -> (String, String) {
    let text = text.trim();
    let at = text
        .rfind(char::is_whitespace)
        .unwrap_or_else(|| panic!("{text:?} is not `type name`"));
    (norm(&text[..at]), text[at..].trim().to_owned())
}

pub(super) fn method(statement: &Statement) -> Method {
    let open = statement.text.find('(').expect("a declaration has `(`");
    assert!(
        statement.text.ends_with(')'),
        "{:?} does not end with `)`",
        statement.text
    );
    let head = statement.text[..open].trim();
    let params = split_params(&statement.text[open + 1..statement.text.len() - 1])
        .iter()
        .map(|p| {
            let (ty, name) = split_typed(p);
            (name, ty)
        })
        .collect();
    let mut throws = false;
    let mut renamed = None;
    for attr in &statement.attrs {
        if attr == "Throws=ProtocolError" {
            throws = true;
        } else if let Some(name) = attr.strip_prefix("Name=") {
            renamed = Some(name.to_owned());
        } else {
            panic!("an unknown method attribute {attr:?} on {head}");
        }
    }
    if head == "constructor" {
        return Method {
            name: renamed.unwrap_or_else(|| "new".to_owned()),
            ret: String::new(),
            params,
            throws,
            constructor: true,
        };
    }
    assert!(
        renamed.is_none(),
        "`Name=` on a method that is not a constructor"
    );
    let (ret, name) = split_typed(head);
    Method {
        name,
        ret,
        params,
        throws,
        constructor: false,
    }
}

pub(super) fn field(statement: &Statement) -> Field {
    assert!(statement.attrs.is_empty(), "a field with attributes");
    let (decl, default) = match statement.text.split_once('=') {
        Some((d, v)) => (d, Some(v.trim().to_owned())),
        None => (statement.text.as_str(), None),
    };
    let (ty, name) = split_typed(decl);
    Field { name, ty, default }
}

fn enum_variants(block: &Block) -> Vec<String> {
    block
        .statements
        .iter()
        .map(|s| {
            s.text
                .strip_prefix('"')
                .and_then(|t| t.strip_suffix('"'))
                .unwrap_or_else(|| {
                    panic!("enum {} has an unquoted variant {:?}", block.name, s.text)
                })
                .to_owned()
        })
        .collect()
}

/// The names the definition gives each kind of type, for validating forms.
struct Universe {
    enums: BTreeSet<String>,
    dictionaries: BTreeSet<String>,
    objects: BTreeSet<String>,
    callbacks: BTreeSet<String>,
}

impl Universe {
    /// Whether a value of `ty` crosses as JSON. Panics on a form nobody
    /// taught the generator, which is the point.
    fn json_type(&self, ty: &str) -> bool {
        let base = ty.strip_suffix('?').unwrap_or(ty);
        if self.objects.contains(base) || self.callbacks.contains(base) {
            return false;
        }
        let known = matches!(
            base,
            "string"
                | "boolean"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "i16"
                | "i32"
                | "i64"
                | "f32"
                | "double"
                | "bytes"
                | "sequence<u8>"
                | "sequence<string>"
                | "record<DOMString,string>"
        ) || self.enums.contains(base)
            || self.dictionaries.contains(base)
            || base
                .strip_prefix("sequence<")
                .and_then(|i| i.strip_suffix('>'))
                .is_some_and(|i| self.dictionaries.contains(i) || self.enums.contains(i));
        assert!(
            known,
            "the type {ty:?} has no JSON form in the C codec; add one to codec.rs \
             and to the generator rather than letting it through"
        );
        true
    }

    /// The Rust expression for a field's declared default.
    fn default_expr(&self, ty: &str, default: &str) -> String {
        match default {
            "null" => {
                assert!(ty.ends_with('?'), "a null default on a non-optional {ty}");
                "None".to_owned()
            }
            "true" | "false" => {
                assert_eq!(ty, "boolean", "a boolean default on {ty}");
                default.to_owned()
            }
            _ if default.starts_with('"') && ty == "string" => format!("{default}.to_owned()"),
            _ if default.parse::<i64>().is_ok()
                && matches!(ty, "u8" | "u16" | "u32" | "u64" | "i16" | "i32" | "i64") =>
            {
                default.to_owned()
            }
            _ if default.parse::<f64>().is_ok() && matches!(ty, "f32" | "double") => {
                if default.contains('.') {
                    default.to_owned()
                } else {
                    format!("{default}.0")
                }
            }
            _ => panic!(
                "the default {default:?} for {ty:?} has no Rust form in the generator; teach \
                 default_expr about it rather than dropping it"
            ),
        }
    }
}

/// The checked-in output path, relative to the crate.
pub(super) const OUTPUT: &str = "src/capi/generated.rs";

/// The generated source for `udl`.
pub(super) fn generate(udl: &str) -> String {
    let blocks = parse(udl);
    let names = |kind: Kind| -> BTreeSet<String> {
        blocks
            .iter()
            .filter(|b| b.kind == kind)
            .map(|b| b.name.clone())
            .collect()
    };
    let universe = Universe {
        enums: names(Kind::Enum),
        dictionaries: names(Kind::Dictionary),
        objects: names(Kind::Interface),
        callbacks: names(Kind::Callback),
    };
    refuse_shared_names(&blocks);

    let mut out = String::new();
    out.push_str(
        "// @generated by `capi::generator` from `offline_protocol.udl`. Do not edit.\n\
         //\n\
         // Regenerate with:\n\
         //   cargo run --manifest-path tools/capi-dispatch-gen/Cargo.toml\n\
         \n\
         //! One dispatch arm per declaration of the interface definition, and the JSON\n\
         //! form of every enum and dictionary in it. See `generator.rs` for why it is\n\
         //! generated and `codec.rs` for the encoding.\n\
         \n\
         // `{ .. }` on every error variant matches unit and tuple variants alike, so\n\
         // the generator needs no knowledge of which is which.\n\
         #![allow(clippy::too_many_lines, clippy::match_same_arms, clippy::unneeded_struct_pattern)]\n\
         \n\
         use serde_json::Value;\n\
         \n\
         use super::codec::{decode_array, encode_array, object, variant_error, CallError, Codec, Decoded, Fields};\n\n",
    );

    let mut hand_written: Vec<String> = Vec::new();
    for block in &blocks {
        match block.kind {
            Kind::Enum => emit_enum(&mut out, block),
            Kind::Dictionary => emit_dictionary(&mut out, block, &universe),
            _ => {}
        }
    }

    for block in &blocks {
        let (prefix, func, receiver) = match (block.kind, block.name.as_str()) {
            (Kind::Namespace, _) => ("", "call_namespace", None),
            (Kind::Interface, "OfflineProtocol") => ("", "call_engine", Some("OfflineProtocol")),
            (Kind::Interface, "MeshServices") => {
                ("services.", "call_services", Some("MeshServices"))
            }
            (Kind::Interface, "DataStore") => ("data.", "call_data", Some("DataStore")),
            (Kind::Interface, other) => panic!(
                "an interface {other} the C binding has no prefix for; give it one here and \
                 in the local API's method table"
            ),
            _ => continue,
        };
        emit_dispatch(
            &mut out,
            block,
            prefix,
            func,
            receiver,
            &universe,
            &mut hand_written,
        );
    }

    let error = blocks
        .iter()
        .find(|b| b.kind == Kind::Enum && b.name == "ProtocolError")
        .expect("the definition declares ProtocolError");
    out.push_str(
        "/// A `ProtocolError`'s variant name and its zero-based position in the\n\
         /// definition. Exhaustive with no fallback arm: a variant added to the Rust\n\
         /// enum and not to the definition, or the other way round, fails to compile.\n\
         pub fn protocol_error_variant(e: &crate::ProtocolError) -> (&'static str, i32) {\n    match e {\n",
    );
    for (i, v) in enum_variants(error).iter().enumerate() {
        let _ = writeln!(
            out,
            "        crate::ProtocolError::{v} {{ .. }} => (\"{v}\", {i}),"
        );
    }
    out.push_str("    }\n}\n\n");

    let storage_error = blocks
        .iter()
        .find(|b| b.kind == Kind::Enum && b.name == "MlsStorageError")
        .expect("the definition declares MlsStorageError");
    out.push_str(
        "/// A C storage function's answer as the definition's `MlsStorageError`:\n\
         /// variant `n` (zero-based) is code `n + 1`, and zero is success. `None` for\n\
         /// any other value, which the adapter reads as the operation's own failure.\n\
         pub fn mls_storage_error_from_code(code: i32) -> Option<crate::MlsStorageError> {\n    match code {\n",
    );
    for (i, v) in enum_variants(storage_error).iter().enumerate() {
        let _ = writeln!(
            out,
            "        {} => Some(crate::MlsStorageError::{v}),",
            i + 1
        );
    }
    out.push_str("        _ => None,\n    }\n}\n\n");

    let namespace = blocks
        .iter()
        .find(|b| b.kind == Kind::Namespace)
        .expect("the definition has a namespace");
    out.push_str("/// The definition's instance-less functions, which need no engine.\npub const NAMESPACE: &[&str] = &[\n");
    for statement in &namespace.statements {
        let m = method(statement);
        if !hand_written.contains(&m.name) {
            let _ = writeln!(out, "    \"{}\",", m.name);
        }
    }
    out.push_str("];\n\n");

    out.push_str(
        "/// Declarations `call` does not dispatch, because a parameter is a callback\n\
         /// or an object, or because they construct one. Each has its own C entry\n\
         /// point or is listed as unavailable in `docs/bridges/c.md`.\n\
         pub const HAND_WRITTEN: &[&str] = &[\n",
    );
    for name in &hand_written {
        let _ = writeln!(out, "    \"{name}\",");
    }
    out.push_str("];\n");
    out
}

fn emit_enum(out: &mut String, block: &Block) {
    if block.name == "ProtocolError" || block.name == "MlsStorageError" {
        return;
    }
    let name = &block.name;
    let variants = enum_variants(block);
    let _ = writeln!(out, "impl Codec for crate::{name} {{");
    out.push_str("    fn decode(value: &Value, at: &str) -> Decoded<Self> {\n        match value.as_str() {\n");
    for v in &variants {
        let _ = writeln!(out, "            Some(\"{v}\") => Ok(Self::{v}),");
    }
    let list = variants
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "            _ => Err(variant_error(at, \"{name}\", &[{list}])),\n        }}\n    }}"
    );
    out.push_str("    fn encode(&self) -> Value {\n        Value::from(match self {\n");
    for v in &variants {
        let _ = writeln!(out, "            Self::{v} => \"{v}\",");
    }
    out.push_str("        })\n    }\n}\n\n");
    emit_vec(out, name);
}

fn emit_vec(out: &mut String, name: &str) {
    let _ = writeln!(
        out,
        "impl Codec for Vec<crate::{name}> {{\n    fn decode(value: &Value, at: &str) -> Decoded<Self> {{\n        decode_array(value, at)\n    }}\n    fn encode(&self) -> Value {{\n        encode_array(self)\n    }}\n}}\n"
    );
}

fn emit_dictionary(out: &mut String, block: &Block, universe: &Universe) {
    let name = &block.name;
    let fields: Vec<Field> = block.statements.iter().map(field).collect();
    let _ = writeln!(out, "impl Codec for crate::{name} {{");
    let _ = writeln!(
        out,
        "    fn decode(value: &Value, at: &str) -> Decoded<Self> {{\n        let mut f = Fields::new(value, at, \"{name}\")?;\n        let out = Self {{"
    );
    for f in &fields {
        assert!(universe.json_type(&f.ty), "{name}.{} is not JSON", f.name);
        match &f.default {
            Some(d) => {
                let expr = universe.default_expr(&f.ty, d);
                let _ = writeln!(
                    out,
                    "            {}: f.take_or(\"{}\", {expr})?,",
                    f.name, f.name
                );
            }
            None => {
                let _ = writeln!(out, "            {}: f.take(\"{}\")?,", f.name, f.name);
            }
        }
    }
    out.push_str("        };\n        f.finish()?;\n        Ok(out)\n    }\n");
    out.push_str("    fn encode(&self) -> Value {\n        object(vec![\n");
    for f in &fields {
        let _ = writeln!(
            out,
            "            (\"{}\", self.{}.encode()),",
            f.name, f.name
        );
    }
    out.push_str("        ])\n    }\n}\n\n");
    emit_vec(out, name);
}

#[allow(clippy::too_many_arguments)]
/// The name `offline_protocol_call` reaches a declaration by.
fn wire_name(prefix: &str, m: &Method) -> String {
    let bare = if m.constructor && m.name == "new" {
        "constructor"
    } else {
        &m.name
    };
    format!("{prefix}{bare}")
}

/// Refuses a definition in which a namespace function and an engine method
/// share a name. Both are called bare, and `call` tries the namespace first,
/// so the engine method would become unreachable from C without any error:
/// the call would silently run the namespace function instead. A hand-written
/// name counts too, because `call` refuses those by name before dispatching.
fn refuse_shared_names(blocks: &[Block]) {
    let bare = |keep: &dyn Fn(&Block) -> bool| -> BTreeSet<String> {
        blocks
            .iter()
            .filter(|b| keep(b))
            .flat_map(|b| b.statements.iter().map(|s| wire_name("", &method(s))))
            .collect()
    };
    let namespace = bare(&|b| b.kind == Kind::Namespace);
    let engine = bare(&|b| b.kind == Kind::Interface && b.name == "OfflineProtocol");
    let shared: Vec<&String> = namespace.intersection(&engine).collect();
    assert!(
        shared.is_empty(),
        "{shared:?} is both a namespace function and an OfflineProtocol method; \
         offline_protocol_call reaches both bare, so rename one in the definition"
    );
}

fn emit_dispatch(
    out: &mut String,
    block: &Block,
    prefix: &str,
    func: &str,
    receiver: Option<&str>,
    universe: &Universe,
    hand_written: &mut Vec<String>,
) {
    let target = match receiver {
        Some(ty) => format!("target: &crate::{ty}, "),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        "/// Dispatches one `{}` declaration by name.\npub fn {func}({target}method: &str, mut args: Fields<'_>) -> Result<Value, CallError> {{\n    match method {{",
        block.name
    );
    for statement in &block.statements {
        let m = method(statement);
        let wire = wire_name(prefix, &m);
        let bridged = !m.constructor
            && m.params.iter().all(|(_, ty)| universe.json_type(ty))
            && (m.ret == "void" || universe.json_type(&m.ret));
        if !bridged {
            hand_written.push(wire);
            continue;
        }
        let _ = writeln!(out, "        \"{}\" => {{", m.name);
        for (name, _) in &m.params {
            let _ = writeln!(out, "            let arg_{name} = args.take(\"{name}\")?;");
        }
        out.push_str("            args.finish()?;\n");
        let call_args = m
            .params
            .iter()
            .map(|(n, _)| format!("arg_{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let callee = match receiver {
            Some(_) => format!("target.{}({call_args})", m.name),
            None => format!("crate::{}({call_args})", m.name),
        };
        let question = if m.throws { "?" } else { "" };
        if m.ret == "void" {
            let _ = writeln!(
                out,
                "            {callee}{question};\n            Ok(Value::Null)\n        }}"
            );
        } else {
            let _ = writeln!(
                out,
                "            let result = {callee}{question};\n            Ok(result.encode())\n        }}"
            );
        }
    }
    let _ = writeln!(
        out,
        "        _ => Err(CallError::MethodNotFound(format!(\"no method {{}}{{method}}\", \"{prefix}\"))),\n    }}\n}}\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDL: &str = include_str!("../offline_protocol.udl");

    #[test]
    fn the_generated_c_dispatch_is_current() {
        let fresh = generate(UDL);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(OUTPUT);
        let current = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        assert!(
            current == fresh,
            "{OUTPUT} is stale against offline_protocol.udl. Regenerate it with \
             `cargo run --manifest-path tools/capi-dispatch-gen/Cargo.toml` and commit the result"
        );
    }

    #[test]
    fn the_parser_sees_every_block_the_definition_declares() {
        let blocks = parse(UDL);
        let count = |k: Kind| blocks.iter().filter(|b| b.kind == k).count();
        let declared = |word: &str| {
            strip_comments(UDL)
                .lines()
                .filter(|l| l.trim_start().starts_with(word))
                .count()
        };
        assert_eq!(count(Kind::Dictionary), declared("dictionary "));
        assert_eq!(count(Kind::Enum), declared("enum "));
        assert_eq!(count(Kind::Callback), declared("callback interface "));
        assert_eq!(count(Kind::Interface), declared("interface "));
        assert_eq!(count(Kind::Namespace), 1);
    }

    /// The panic message of `generate(udl)`, which must panic.
    fn refusal(udl: &'static str) -> String {
        let payload = std::panic::catch_unwind(|| generate(udl))
            .expect_err("the generator accepted what it cannot write");
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default()
    }

    const ERRORS: &str = "[Error] enum ProtocolError { \"Other\" };";

    #[test]
    fn an_unknown_type_form_stops_the_generator() {
        let udl: &'static str = Box::leak(
            format!("namespace n {{ void f(sequence<sequence<string>> x); }}; {ERRORS}")
                .into_boxed_str(),
        );
        assert!(
            refusal(udl).contains("has no JSON form"),
            "{}",
            refusal(udl)
        );
    }

    #[test]
    fn a_namespace_function_named_like_an_engine_method_stops_the_generator() {
        let udl: &'static str = Box::leak(
            format!(
                "namespace n {{ string version(); }}; \
                 interface OfflineProtocol {{ string version(); }}; {ERRORS}"
            )
            .into_boxed_str(),
        );
        assert!(
            refusal(udl).contains("both a namespace function and an OfflineProtocol method"),
            "{}",
            refusal(udl)
        );
    }

    #[test]
    fn an_unknown_default_stops_the_generator() {
        let udl: &'static str = Box::leak(
            format!("namespace n {{}}; dictionary D {{ u32 x = [1]; }}; {ERRORS}").into_boxed_str(),
        );
        assert!(
            refusal(udl).contains("has no Rust form"),
            "{}",
            refusal(udl)
        );
    }
}
