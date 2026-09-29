//! The store handle, and the one place that decides what it is.
//!
//! # One handle type per target, named in one file
//!
//! A [`LeafDevice`](crate::LeafDevice) and the adapters it builds share one
//! store, so the handle is reference counted. Where the processor has atomic
//! compare-and-swap that is `alloc::sync::Arc`. Where it has none (ESP32-C3,
//! ESP32-C2, every Cortex-M0) `alloc::sync` does not exist, and the handle is
//! `portable_atomic_util::Arc` instead, the same choice the pinned mls-rs makes
//! for its own handles.
//!
//! The failure this file prevents is a build that works on every machine a
//! developer owns and nowhere a device runs: a host always has atomics, so an
//! `alloc::sync` import anywhere else in this crate compiles, passes every
//! test, and breaks only the bare-metal targets. `only_one_file_names_the_counted_pointer`
//! in this module refuses any route to `alloc::sync` outside this file on the
//! host (a path, an import in any form, a glob over `alloc`, or a renamed
//! `alloc`), and the `embedded-core` CI job refuses it on the targets.
//!
//! # Building a handle
//!
//! [`shared_store`] is the portable way. The replacement pointer cannot turn
//! an `Arc<MyStore>` into an `Arc<dyn LeafStore>` on stable Rust, so the
//! constructor goes through a `Box<dyn LeafStore>`, which costs one extra
//! allocation and one copy of the store value, once, at boot. It takes that
//! route on every target, so the host tests run the constructor a device runs.
//! They do not run the replacement pointer: a host always has atomics, so its
//! tests always get `alloc::sync::Arc`, and `portable_atomic_util::Arc` is
//! compiled on the targets without compare-and-swap and exercised nowhere.
//!
//! On a target with atomics [`SharedStore`] is `alloc::sync::Arc<dyn LeafStore>`,
//! which is `std::sync::Arc` wherever `std` exists, so code that builds one with
//! `Arc::new` keeps compiling there. On a target without, only [`shared_store`]
//! builds one.
//!
//! # What a target without compare-and-swap owes
//!
//! A critical-section implementation, linked by the firmware. The reference
//! count is only as sound as it is: an implementation that does not actually
//! exclude interrupts or other cores makes the count racy, and a racy count is
//! a use-after-free of the store.

use alloc::boxed::Box;

#[cfg(target_has_atomic = "ptr")]
pub(crate) use alloc::sync::Arc;
#[cfg(not(target_has_atomic = "ptr"))]
pub(crate) use portable_atomic_util::Arc;

use crate::store::LeafStore;

/// A shared handle to the device's store.
///
/// On a target with atomic compare-and-swap this is `alloc::sync::Arc`, the
/// same type as `std::sync::Arc`. On one without, it is
/// `portable_atomic_util::Arc`. Build one with [`shared_store`] to write code
/// that compiles on both.
pub type SharedStore = Arc<dyn LeafStore>;

/// Wraps a store in the handle a [`LeafDevice`](crate::LeafDevice) takes.
///
/// ```
/// use offline_protocol_leaf::{shared_store, LeafDevice, MemoryStore};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let device = LeafDevice::open(shared_store(MemoryStore::new()), "com.example.lock")?;
/// # let _ = device;
/// # Ok(())
/// # }
/// ```
pub fn shared_store<S: LeafStore + 'static>(store: S) -> SharedStore {
    let boxed: Box<dyn LeafStore> = Box::new(store);
    Arc::from(boxed)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, KEY_TYPE_IDENTITY};
    use crate::LeafDevice;
    use std::string::String;
    use std::vec::Vec;
    use std::{format, fs, path::PathBuf};

    #[test]
    fn a_store_built_by_the_constructor_is_the_one_the_device_writes() {
        let store = shared_store(MemoryStore::new());
        let reader = Arc::clone(&store);
        let _device = LeafDevice::open(store, "com.example.lock").expect("device opens");

        let identity = reader
            .load(KEY_TYPE_IDENTITY, "signature_public")
            .expect("load succeeds");
        assert!(
            identity.is_some(),
            "provisioning must write through the handle the constructor built"
        );
    }

    /// Splits Rust source into tokens, dropping whitespace, comments, string,
    /// byte-string, raw-string and character literals, and lifetimes, so that
    /// a path mentioned in prose or in a literal is never read as code. `::`
    /// is one token; every other punctuation character is its own.
    fn tokens(src: &str) -> Vec<String> {
        let chars: Vec<char> = src.chars().collect();
        let ident = |c: char| c == '_' || c.is_alphanumeric();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c.is_whitespace() {
                i += 1;
            } else if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 0usize;
                while i < chars.len() {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            } else if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            } else if c == '\'' {
                if next == Some('\\') {
                    i += 2;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                } else if chars.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    // A lifetime or a label: skip the quote and its name.
                    i += 1;
                    while i < chars.len() && ident(chars[i]) {
                        i += 1;
                    }
                }
            } else if ident(c) {
                let begin = i;
                while i < chars.len() && ident(chars[i]) {
                    i += 1;
                }
                let word: String = chars[begin..i].iter().collect();
                let raw_prefix = word == "r" || word == "br";
                let hashes = chars[i..].iter().take_while(|&&h| h == '#').count();
                if raw_prefix && chars.get(i + hashes) == Some(&'"') {
                    // A raw string: skip to the quote followed by as many hashes.
                    i += hashes + 1;
                    while i < chars.len() {
                        if chars[i] == '"'
                            && chars[i + 1..].iter().take_while(|&&h| h == '#').count() >= hashes
                        {
                            i += 1 + hashes;
                            break;
                        }
                        i += 1;
                    }
                } else if word == "b" && matches!(chars.get(i), Some('"') | Some('\'')) {
                    // A byte string or byte literal: the next turn skips it.
                } else if word == "r" && hashes == 1 && chars.get(i + 1).is_some_and(|&n| ident(n))
                {
                    // A raw identifier, `r#sync`: the next turn reads the name.
                    i += 1;
                } else {
                    out.push(word);
                }
            } else if c == ':' && next == Some(':') {
                out.push("::".into());
                i += 2;
            } else {
                out.push(c.to_string());
                i += 1;
            }
        }
        out
    }

    /// Flattens one `use` tree starting at `tokens[*at]` into full paths, each
    /// with the name it is imported under when that is renamed. `self` inside
    /// a group names the group's own path.
    fn use_tree(
        tokens: &[String],
        at: &mut usize,
        prefix: Vec<String>,
        out: &mut Vec<(Vec<String>, Option<String>)>,
    ) {
        let mut path = prefix;
        while let Some(token) = tokens.get(*at) {
            match token.as_str() {
                "::" => *at += 1,
                "{" => {
                    *at += 1;
                    while let Some(inner) = tokens.get(*at) {
                        match inner.as_str() {
                            "}" => {
                                *at += 1;
                                break;
                            }
                            "," => *at += 1,
                            ";" => return,
                            _ => {
                                let before = *at;
                                use_tree(tokens, at, path.clone(), out);
                                if *at == before {
                                    *at += 1;
                                }
                            }
                        }
                    }
                    return;
                }
                "*" => {
                    *at += 1;
                    path.push("*".into());
                    out.push((path, None));
                    return;
                }
                word if word.starts_with(|c: char| c == '_' || c.is_alphabetic()) => {
                    *at += 1;
                    if word != "self" {
                        path.push(word.into());
                    }
                    if tokens.get(*at).is_some_and(|t| t == "::") {
                        continue;
                    }
                    let rename = (tokens.get(*at).is_some_and(|t| t == "as"))
                        .then(|| tokens.get(*at + 1).cloned())
                        .flatten();
                    if rename.is_some() {
                        *at += 2;
                    }
                    out.push((path, rename));
                    return;
                }
                _ => return,
            }
        }
    }

    /// Every place in `src` that reaches the `alloc` crate's `sync` module, or
    /// could: a path through it in code or in an import, a glob over the
    /// `alloc` root (which imports `sync` by name), and the same through any
    /// name `alloc` is renamed to. `std::sync` and `core::sync` are other
    /// modules and are never reported, and neither is a mention in a comment
    /// or a literal.
    ///
    /// This matches the class rather than a list of line shapes. The first two
    /// versions of this guard matched shapes, and each missed one rustfmt or a
    /// person could write (`sync as s`, `sync::*`, a multi-line group).
    fn counted_pointer_paths(src: &str) -> Vec<String> {
        let tokens = tokens(src);
        let mut imports = Vec::new();
        let mut aliases = vec![String::from("alloc")];
        for (index, token) in tokens.iter().enumerate() {
            if token == "use" {
                use_tree(&tokens, &mut (index + 1), Vec::new(), &mut imports);
            }
            let rest = &tokens[index..];
            if rest.len() >= 5
                && rest[0] == "extern"
                && rest[1] == "crate"
                && rest[2] == "alloc"
                && rest[3] == "as"
            {
                aliases.push(rest[4].clone());
            }
        }
        // `use alloc as heap;` and `use alloc::{self as heap};` rename it too,
        // and a rename of a rename counts, so loop until nothing new appears.
        loop {
            let found: Vec<String> = imports
                .iter()
                .filter(|(path, rename)| {
                    path.len() == 1 && aliases.contains(&path[0]) && rename.is_some()
                })
                .filter_map(|(_, rename)| rename.clone())
                .filter(|name| !aliases.contains(name))
                .collect();
            if found.is_empty() {
                break;
            }
            aliases.extend(found);
        }
        let through_sync = |root: &str, second: &str| {
            aliases.iter().any(|alias| alias == root) && (second == "sync" || second == "*")
        };

        let mut offenders: Vec<String> = imports
            .iter()
            .filter(|(path, _)| path.len() >= 2 && through_sync(&path[0], &path[1]))
            .map(|(path, _)| format!("use {}", path.join("::")))
            .collect();
        offenders.extend(
            tokens
                .windows(3)
                .filter(|w| w[1] == "::" && w[2] == "sync" && through_sync(&w[0], &w[2]))
                .map(|w| format!("{}::sync", w[0])),
        );
        offenders
    }

    fn rust_files(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{} is unreadable: {error}", dir.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|error| {
                    panic!("an entry in {} is unreadable: {error}", dir.display())
                })
                .path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    #[test]
    fn the_guard_sees_every_route_to_the_counted_pointer() {
        for code in [
            "use alloc::sync::Arc;",
            "use alloc::sync;",
            "use ::alloc::sync::Weak;",
            "use alloc::{string::String, sync::Arc};",
            "use alloc::{sync::{Arc, Weak}, vec::Vec};",
            "use alloc::{string::String, sync as s};",
            "use alloc::{string::String, sync::*};",
            "use alloc::*;",
            "use alloc::{\n    string::String,\n    sync::Arc,\n};",
            "pub(crate) use alloc::sync::Arc;",
            "let store: alloc::sync::Arc<dyn LeafStore>;",
            "extern crate alloc as heap;\nuse heap::sync::Arc;",
            "use alloc as heap;\nfn f(_: heap::sync::Arc<u8>) {}",
            "use alloc::{self as heap};\nuse heap::sync::Arc;",
            "let quote = '\"'; use alloc::sync::Arc;",
            "fn f<'a>(x: &'a str) -> &'a str { x }\nuse alloc::sync::Arc;",
            "const S: &[u8] = b\"x\"; use alloc::sync::Arc;",
            "const R: &str = r#\"a \" b\"#; use alloc::sync::Arc;",
        ] {
            assert!(!counted_pointer_paths(code).is_empty(), "missed: {code}");
        }
        for code in [
            "use std::sync::Mutex;",
            "use std::sync::{Mutex, MutexGuard};",
            "use std::{sync::Mutex, vec::Vec};",
            "use core::sync::atomic::{AtomicU32, Ordering};",
            "use crate::shared::Arc;",
            "use alloc::{string::String, vec::Vec};",
            "let n = 1; // alloc::sync is gone here",
            "/* use alloc::sync::Arc; /* nested */ still a comment */",
            "const S: &str = \"use alloc::sync::Arc;\";",
            "const R: &str = r#\"use alloc::sync::Arc;\"#;",
            "let sync = true; call(a, sync); file.sync_all();",
            "mod sync { pub struct Arc; } use self::sync::Arc;",
        ] {
            assert_eq!(
                counted_pointer_paths(code),
                Vec::<String>::new(),
                "false alarm: {code}"
            );
        }
    }

    #[test]
    fn only_one_file_names_the_counted_pointer() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let exempt = src.join("shared.rs");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        assert!(
            files.contains(&exempt),
            "the guard did not find {}",
            exempt.display()
        );

        // An unreadable file fails the test: a guard that skips what it cannot
        // read reports clean for code it never saw.
        let offenders: Vec<String> = files
            .iter()
            .filter(|path| **path != exempt)
            .flat_map(|path| {
                let text = fs::read_to_string(path)
                    .unwrap_or_else(|error| panic!("{} is unreadable: {error}", path.display()));
                counted_pointer_paths(&text)
                    .into_iter()
                    .map(move |found| format!("{}: {found}", path.display()))
            })
            .collect();

        assert!(
            offenders.is_empty(),
            "these reach alloc::sync directly, which compiles on every host and \
             breaks every target without compare-and-swap; take `Arc` from \
             `crate::shared` instead: {offenders:?}"
        );
    }
}
