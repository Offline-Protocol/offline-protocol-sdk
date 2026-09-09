//! Where the core's `tracing` output goes on a device.
//!
//! The core logs through `tracing` and installs no subscriber, so on a phone
//! its output went nowhere. The telemetry pipe's debug tap (one `debug` line
//! per wire event, under [`PIPE_LOG_TARGET`]) needs somewhere to land, and
//! this is it: a `log` logger installed once per process that writes to the
//! platform log (logcat on Android, the unified log on iOS) or to stderr on
//! a desktop host. `tracing`'s `log` feature turns every `tracing` macro in
//! the workspace into a `log` record when no subscriber is set, which is the
//! case here by construction.
//!
//! Two levels. Everything at `info` and above is always passed through; the
//! pipe's target is raised to `debug` only while a pipe runs with
//! `debug: true`, so an application that never asks for the tap pays
//! nothing for it and sees nothing from it. Nothing is ever delivered to
//! application code: the tap is an inspection aid for the developer holding
//! the device, not an event stream.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use log::{Level, LevelFilter, Log, Metadata, Record};
use offline_protocol::telemetry::pipe::PIPE_LOG_TARGET;

static PIPE_DEBUG: AtomicBool = AtomicBool::new(false);
static INSTALL: Once = Once::new();

/// Installs the host logger. Idempotent; a logger some other crate already
/// installed is left in place.
pub(crate) fn install_host_logger() {
    INSTALL.call_once(|| {
        let logger = HostLogger {
            inner: platform_logger(),
        };
        if log::set_boxed_logger(Box::new(logger)).is_ok() {
            log::set_max_level(LevelFilter::Debug);
        }
    });
}

/// Raises or lowers the pipe's target to `debug`.
pub(crate) fn set_pipe_debug(enabled: bool) {
    PIPE_DEBUG.store(enabled, Ordering::Relaxed);
}

struct HostLogger {
    inner: Box<dyn Log>,
}

impl Log for HostLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if metadata.level() <= Level::Info {
            return true;
        }
        metadata.level() == Level::Debug
            && metadata.target().starts_with(PIPE_LOG_TARGET)
            && PIPE_DEBUG.load(Ordering::Relaxed)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(target_os = "android")]
fn platform_logger() -> Box<dyn Log> {
    Box::new(android_logger::AndroidLogger::new(
        android_logger::Config::default()
            .with_max_level(LevelFilter::Debug)
            .with_tag("OfflineProtocol"),
    ))
}

#[cfg(target_os = "ios")]
fn platform_logger() -> Box<dyn Log> {
    Box::new(oslog::OsLogger::new("com.offlineprotocol.sdk").level_filter(LevelFilter::Debug))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn platform_logger() -> Box<dyn Log> {
    Box::new(StderrLogger)
}

/// The desktop logger: one line per record on stderr, which is where a
/// Python host or a Rust embedder expects a library to write.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct StderrLogger;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Log for StderrLogger {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(
            stderr,
            "[offline-protocol] {} {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {
        use std::io::Write;
        let _ = std::io::stderr().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_target_is_debug_only_while_a_pipe_asks_for_it() {
        struct Silent;
        impl Log for Silent {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn log(&self, _: &Record<'_>) {}
            fn flush(&self) {}
        }
        let logger = HostLogger {
            inner: Box::new(Silent),
        };
        let pipe_debug = Metadata::builder()
            .level(Level::Debug)
            .target(PIPE_LOG_TARGET)
            .build();
        let other_debug = Metadata::builder()
            .level(Level::Debug)
            .target("offline_protocol::protocol")
            .build();
        let other_info = Metadata::builder()
            .level(Level::Info)
            .target("offline_protocol::protocol")
            .build();

        set_pipe_debug(false);
        assert!(!logger.enabled(&pipe_debug));
        assert!(!logger.enabled(&other_debug));
        assert!(logger.enabled(&other_info));

        set_pipe_debug(true);
        assert!(logger.enabled(&pipe_debug));
        assert!(
            !logger.enabled(&other_debug),
            "only the pipe's target is raised"
        );
        set_pipe_debug(false);
    }
}
