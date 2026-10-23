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
        let forbidden = ["Instant", "SystemTime", "UNIX_EPOCH"];
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
                for (number, line) in source.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    let mut rest = code;
                    while let Some(start) = rest.find("std::time::") {
                        rest = &rest[start + "std::time::".len()..];
                        // `std::time::{A, B}` names everything up to the brace;
                        // `std::time::A` names one identifier.
                        let named = match rest.strip_prefix('{') {
                            Some(group) => group.split('}').next().unwrap_or(group),
                            None => rest
                                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                                .next()
                                .unwrap_or(""),
                        };
                        let clock = named.split(',').any(|item| {
                            let item = item.trim().split(" as ").next().unwrap_or("").trim();
                            forbidden.contains(&item)
                        });
                        if clock {
                            offenders.push(format!(
                                "{}:{}: {}",
                                path.display(),
                                number + 1,
                                line.trim()
                            ));
                            break;
                        }
                    }
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
