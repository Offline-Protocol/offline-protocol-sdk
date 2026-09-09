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
//! One target, two levels. Only records under [`PIPE_LOG_TARGET`] are passed
//! through at all: the pipe's own `info`, `warn` and `error` always, and its
//! `debug` — the per-wire-event tap — while a pipe runs with `debug: true`.
//! Everything else the workspace logs is dropped here, which is what makes
//! good the promise that an application that never asks for the tap pays
//! nothing for it and sees nothing from it.
//!
//! That filter is a privacy boundary rather than a volume control, and it is
//! why it is not spelled as a level. Until 0.26 the core wrote nothing to a
//! device log; the engine's own `info!` and `warn!` lines name groups,
//! senders and peers, which is precisely what the scrubber exists to keep out
//! of telemetry, and a device log is readable by anything on the device that
//! can run `logcat`. Passing them through because they happen to sit above a
//! level threshold would be a disclosure the application never asked for and
//! cannot switch off. An embedder that does want the whole stream installs
//! its own `log` or `tracing` subscriber, which [`install_host_logger`]
//! leaves in place.
//!
//! Nothing is ever delivered to application code: the tap is an inspection
//! aid for the developer holding the device, not an event stream.

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
        // The pipe's target and nothing else; see the module docs for why
        // this is a target filter and not a level filter.
        if !metadata.target().starts_with(PIPE_LOG_TARGET) {
            return false;
        }
        if metadata.level() <= Level::Info {
            return true;
        }
        metadata.level() == Level::Debug && PIPE_DEBUG.load(Ordering::Relaxed)
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

    /// A sink that accepts everything, so what these tests measure is
    /// [`HostLogger::enabled`] and nothing beneath it.
    struct Silent;
    impl Log for Silent {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn log(&self, _: &Record<'_>) {}
        fn flush(&self) {}
    }

    /// The privacy boundary: the engine's own output never reaches the
    /// device log, at any level. Reverting the target check turns every
    /// `info!` and `warn!` in the workspace — which name groups, senders and
    /// peers — into logcat lines that no application setting can stop.
    #[test]
    fn nothing_outside_the_pipes_target_ever_reaches_the_host_log() {
        let logger = HostLogger {
            inner: Box::new(Silent),
        };
        for level in [
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            for target in [
                "offline_protocol::protocol",
                "offline_protocol::protocol::group_mesh",
                "offline_protocol_transport::ble",
                "ureq",
            ] {
                for tap in [false, true] {
                    set_pipe_debug(tap);
                    let metadata = Metadata::builder().level(level).target(target).build();
                    assert!(
                        !logger.enabled(&metadata),
                        "{level} on {target} reached the host log (tap={tap})"
                    );
                }
            }
        }
        set_pipe_debug(false);
    }

    /// The pipe's own warnings are not gated on the tap: a batch that could
    /// not be persisted is the thing a developer is holding the device to
    /// see, and it fires whether or not `debug` was asked for.
    #[test]
    fn the_pipes_own_warnings_pass_without_the_tap() {
        let logger = HostLogger {
            inner: Box::new(Silent),
        };
        set_pipe_debug(false);
        for level in [Level::Error, Level::Warn, Level::Info] {
            let metadata = Metadata::builder()
                .level(level)
                .target(PIPE_LOG_TARGET)
                .build();
            assert!(logger.enabled(&metadata), "{level}");
        }
    }

    #[test]
    fn the_pipe_target_is_debug_only_while_a_pipe_asks_for_it() {
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
        assert!(
            !logger.enabled(&other_info),
            "the engine's own output is not the pipe's tap"
        );

        set_pipe_debug(true);
        assert!(logger.enabled(&pipe_debug));
        assert!(
            !logger.enabled(&other_debug),
            "only the pipe's target is raised"
        );
        set_pipe_debug(false);
    }
}
