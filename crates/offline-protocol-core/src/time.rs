//! The workspace's clock.
//!
//! Every crate in the workspace reads the time through this module and never
//! through `std::time::Instant` or `std::time::SystemTime` directly. On every
//! target but one, this module is `std::time` and nothing else: the types are
//! the standard library's own, so a value crosses any API that names
//! `std::time` unchanged.
//!
//! The exception is `wasm32-unknown-unknown`, the browser target. There the
//! standard library compiles `Instant::now()` and `SystemTime::now()` and
//! makes both panic when called, because the target defines no host clock. A
//! green build therefore says nothing; the first tick of the engine aborts the
//! module. On that target this module re-exports `web-time`, which reads
//! `performance.now()` and `Date.now()` and has the same API.
//!
//! `clocks_are_read_through_core_time_only` in this crate walks every crate's
//! sources and refuses a direct `std::time` clock import, because nothing else
//! notices one until it runs in a browser.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::{Instant, SystemTime, SystemTimeError, UNIX_EPOCH};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use web_time::{Instant, SystemTime, SystemTimeError, UNIX_EPOCH};

pub use core::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_is_std_time_on_a_host() {
        // A host must get the standard library's own types, so a value from
        // here satisfies any API that names `std::time`.
        let instant: std::time::Instant = Instant::now();
        let wall: std::time::SystemTime = SystemTime::now();
        assert!(instant.elapsed() < Duration::from_secs(60));
        assert!(wall.duration_since(UNIX_EPOCH).is_ok());
    }

    /// The clock types `std::time` must not be reached for directly.
    const FORBIDDEN: [&str; 3] = ["Instant", "SystemTime", "UNIX_EPOCH"];

    /// Every place in `source` that names a `std::time` clock, in any of the
    /// spellings Rust accepts: `std::time::Instant`, `std::time::{A, B}`,
    /// `std::{time::Instant, ..}` at any depth, a renaming `as`, or the module
    /// itself (`use std::time;`, `std::time::{self}`), which is the clock one
    /// path segment later. Comments are dropped and whitespace around path
    /// punctuation is removed first, so a group split over lines is seen
    /// whole.
    fn clock_imports(source: &str) -> Vec<String> {
        let code: String = source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        let code = tighten(&code);
        let mut found = Vec::new();
        let mut rest = code.as_str();
        while let Some(start) = rest.find("std::") {
            let tail = &rest[start + "std::".len()..];
            // `core::time` and `alloc::...` end in `std::` only as part of a
            // longer name; require a path boundary before `std`.
            let boundary = rest[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if boundary && names_a_clock(tail) {
                let end = tail.find(';').unwrap_or(tail.len()).min(80);
                found.push(format!("std::{}", &tail[..end]));
            }
            rest = tail;
        }
        found
    }

    /// Collapses whitespace, and drops it entirely next to `:`, `{`, `}`,
    /// `,` and `;`, so `std::{\n    time::Instant,\n}` reads `std::{time::Instant,}`.
    fn tighten(code: &str) -> String {
        let collapsed: Vec<&str> = code.split_whitespace().collect();
        let mut out = String::new();
        for word in collapsed {
            let glue = |c: char| matches!(c, ':' | '{' | '}' | ',' | ';');
            let joins =
                out.chars().next_back().is_some_and(glue) || word.chars().next().is_some_and(glue);
            if !out.is_empty() && !joins {
                out.push(' ');
            }
            out.push_str(word);
        }
        out
    }

    /// Whether the path after a `std::` names a clock.
    fn names_a_clock(tail: &str) -> bool {
        if let Some(rest) = tail.strip_prefix("time::") {
            return match rest.strip_prefix('{') {
                Some(group) => top_level_items(group).iter().any(|item| {
                    let item = item.split(" as ").next().unwrap_or("").trim();
                    item == "self" || FORBIDDEN.contains(&item)
                }),
                None => FORBIDDEN.contains(&ident(rest)),
            };
        }
        if let Some(rest) = tail.strip_prefix("time") {
            // The module itself: `std::time;`, `std::time as t`, or an item
            // of a group, `std::{time, ..}`.
            return rest.is_empty() || rest.starts_with([';', ',', '}', ' ']);
        }
        match tail.strip_prefix('{') {
            Some(group) => top_level_items(group)
                .iter()
                .any(|item| names_a_clock(item)),
            None => false,
        }
    }

    /// The leading identifier of `text`.
    fn ident(text: &str) -> &str {
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        &text[..end]
    }

    /// The comma-separated items of a brace group whose opening brace is
    /// already consumed, split at depth zero only, up to the matching brace.
    fn top_level_items(group: &str) -> Vec<&str> {
        let mut items = Vec::new();
        let (mut depth, mut from) = (0usize, 0usize);
        for (at, c) in group.char_indices() {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => {
                    items.push(&group[from..at]);
                    return items;
                }
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    items.push(&group[from..at]);
                    from = at + 1;
                }
                _ => {}
            }
        }
        items.push(&group[from..]);
        items
    }

    #[test]
    fn the_clock_guard_sees_every_spelling() {
        for named in [
            "let now = std::time::Instant::now();",
            "use std::time::{Duration, SystemTime};",
            "use std::time::{UNIX_EPOCH as EPOCH};",
            "use std::time::{self};",
            "use std::time;",
            "use std::{collections::HashMap, time::Instant};",
            "use std::{\n    sync::Arc,\n    time::{\n        Duration,\n        Instant,\n    },\n};",
            "use std::{sync::{Arc, Mutex}, time};",
        ] {
            assert!(!clock_imports(named).is_empty(), "missed: {named}");
        }
        for clean in [
            "use std::time::Duration;",
            "use std::{collections::HashMap, time::Duration};",
            "use core::time::Duration;",
            "use offline_protocol_core::time::Instant;",
            "// std::time::Instant in a comment",
            "let timeout = std::time::Duration::from_secs(1);",
        ] {
            assert!(clock_imports(clean).is_empty(), "false alarm: {clean}");
        }
    }

    /// Every crate takes its clock from this module. A direct
    /// `std::time::Instant` or `std::time::SystemTime` compiles and passes on
    /// every host, and panics on the first call in a browser.
    #[test]
    fn clocks_are_read_through_core_time_only() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        // A published .crate archive has no sibling crates. Skip rather than
        // fail: the check belongs to the workspace, not the package.
        if !crates.join("offline-protocol").is_dir() {
            return;
        }
        let mut offenders = Vec::new();
        // The crates a browser build compiles. The FFI crate and the benches
        // never run there, and the leaf has its own clock contract (a time
        // source is a provisioning obligation, ADR 0021).
        let browser_crates = [
            "offline-protocol-core",
            "offline-protocol-sealed",
            "offline-protocol-transport",
            "offline-protocol-reliability",
            "offline-protocol-mls",
            "offline-protocol-services",
            "offline-protocol-data",
            "offline-protocol-router",
            "offline-protocol",
            "offline-protocol-web",
        ];
        let mut stack: Vec<_> = browser_crates
            .iter()
            .map(|name| crates.join(name).join("src"))
            .collect();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !name.ends_with(".rs") || path.ends_with("offline-protocol-core/src/time.rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for fragment in clock_imports(&source) {
                    offenders.push(format!("{}: {fragment}", path.display()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "read the clock through `offline_protocol_core::time`, not `std::time` \
             (it panics on wasm32-unknown-unknown):\n{}",
            offenders.join("\n")
        );
    }
}
