//! What the host tells the pipe about itself: the platform every batch
//! envelope names, and the application lifecycle the session boundaries
//! follow.
//!
//! The binding fills these in, never the application: an `os` the app could
//! set is an `os` the app could set wrong, and the ingest keys its
//! per-platform aggregates on it.

use offline_protocol_telemetry_wire::OsKind;

/// The operating system the SDK is running on, as the ingest names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryOs {
    /// Apple iOS.
    Ios,
    /// Android.
    Android,
    /// Linux.
    Linux,
    /// Apple macOS.
    Macos,
    /// Microsoft Windows.
    Windows,
    /// A host the binding could not classify.
    Other,
}

impl TelemetryOs {
    /// The host this binary was compiled for, with no version information.
    ///
    /// The bindings pass the real major version alongside; this is the
    /// fallback for a Rust embedder that has nothing better to say.
    pub fn current() -> Self {
        if cfg!(target_os = "ios") {
            Self::Ios
        } else if cfg!(target_os = "android") {
            Self::Android
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }

    pub(crate) fn wire(self) -> OsKind {
        match self {
            Self::Ios => OsKind::Ios,
            Self::Android => OsKind::Android,
            Self::Linux => OsKind::Linux,
            Self::Macos => OsKind::Macos,
            Self::Windows => OsKind::Windows,
            Self::Other => OsKind::Other,
        }
    }

    pub(crate) fn user_agent_token(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Other => "other",
        }
    }
}

/// The application lifecycle states the pipe draws session boundaries from.
///
/// `Background` closes a session (summary, then a flush); the first `Active`
/// after a `Background` opens the next one. `Inactive` is the transient state
/// iOS reports around a notification or an incoming call and is ignored: it
/// is not a boundary a user would recognise as leaving the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    /// The application is in the foreground.
    Active,
    /// The application has left the foreground.
    Background,
    /// A transient foreground interruption; ignored.
    Inactive,
}

/// Host facts supplied at `enable_telemetry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryHost {
    /// The host operating system.
    pub os: TelemetryOs,
    /// Its major version (`18` for iOS 18.x; the API level on Android).
    pub os_major: u16,
    /// The application state at the moment telemetry is enabled, so a pipe
    /// enabled by a background launch closes its first session on the next
    /// foreground rather than waiting for a round trip it may never see.
    pub app_state: AppState,
}

impl TelemetryHost {
    /// A host that is active right now.
    pub fn new(os: TelemetryOs, os_major: u16) -> Self {
        Self {
            os,
            os_major,
            app_state: AppState::Active,
        }
    }

    /// Sets the application state at enable time.
    pub fn with_app_state(mut self, app_state: AppState) -> Self {
        self.app_state = app_state;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_os_has_a_wire_form_and_a_user_agent_token() {
        for os in [
            TelemetryOs::Ios,
            TelemetryOs::Android,
            TelemetryOs::Linux,
            TelemetryOs::Macos,
            TelemetryOs::Windows,
            TelemetryOs::Other,
        ] {
            let wire = serde_json::to_value(os.wire()).expect("serializes");
            assert_eq!(
                wire,
                serde_json::Value::String(os.user_agent_token().into())
            );
        }
    }

    #[test]
    fn current_platform_is_one_of_the_named_hosts() {
        // On the machines this suite runs on, the answer is never `Other`.
        assert_ne!(TelemetryOs::current(), TelemetryOs::Other);
    }
}
