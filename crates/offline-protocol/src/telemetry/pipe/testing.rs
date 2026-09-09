//! Test seams for the pipe, compiled only under the `test-utils` feature.
//!
//! The pipe has no runtime hook an application could repoint (the endpoint is
//! a build-time constant and the HTTP client is not a parameter). Tests in
//! other crates still need to see what leaves the device without a network,
//! so this module lets a test process capture every upload instead of
//! sending it. The feature is enabled by dev-dependencies only, never by a
//! shipped binary, which is the same arrangement the transport crate uses
//! for its mock transport.

use std::sync::{Arc, Mutex, OnceLock};

use super::uploader::{HttpClient, Request, Response};

/// One captured upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedUpload {
    /// The request headers, in the order they were set.
    pub headers: Vec<(String, String)>,
    /// The request body: one serialized batch.
    pub body: String,
}

#[derive(Debug)]
struct CaptureState {
    status: u16,
    uploads: Vec<CapturedUpload>,
}

/// A stand-in for the HTTPS client that records every upload and answers
/// with a configurable status. Cloning shares the recording.
#[derive(Debug, Clone)]
pub struct CapturingHttpClient {
    inner: Arc<Mutex<CaptureState>>,
}

impl Default for CapturingHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CapturingHttpClient {
    /// A client that answers every upload with `202`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CaptureState {
                status: 202,
                uploads: Vec::new(),
            })),
        }
    }

    /// Sets the status every later upload is answered with. `0` means no
    /// answer at all: the uploader sees a transport error.
    pub fn set_status(&self, status: u16) {
        self.lock().status = status;
    }

    /// Every upload captured so far, oldest first.
    pub fn uploads(&self) -> Vec<CapturedUpload> {
        self.lock().uploads.clone()
    }

    /// The bodies captured so far, oldest first.
    pub fn bodies(&self) -> Vec<String> {
        self.lock().uploads.iter().map(|u| u.body.clone()).collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl HttpClient for CapturingHttpClient {
    fn post(&mut self, request: &Request<'_>) -> Result<Response, String> {
        let mut state = self.lock();
        if state.status == 0 {
            return Err("capturing client: no answer".into());
        }
        state.uploads.push(CapturedUpload {
            headers: request
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: request.body.to_string(),
        });
        let accepted = serde_json::from_str::<serde_json::Value>(request.body)
            .ok()
            .and_then(|v| v["events"].as_array().map(Vec::len))
            .unwrap_or(0);
        Ok(Response {
            status: state.status,
            retry_after: None,
            body: format!(r#"{{"accepted":{accepted},"rejected":0,"rejected_reasons":[]}}"#),
        })
    }
}

static OVERRIDE: OnceLock<Mutex<Option<CapturingHttpClient>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<CapturingHttpClient>> {
    OVERRIDE.get_or_init(|| Mutex::new(None))
}

/// Routes every upload from every pipe started after this call into the
/// returned client instead of the network. Process-wide; call
/// [`stop_capturing_uploads`] to restore the real client.
pub fn capture_uploads() -> CapturingHttpClient {
    let client = CapturingHttpClient::new();
    *slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(client.clone());
    client
}

/// Restores the real HTTPS client for pipes started after this call.
pub fn stop_capturing_uploads() {
    *slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn override_client() -> Option<Box<dyn HttpClient>> {
    slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .map(|client| Box::new(client) as Box<dyn HttpClient>)
}

/// Starts a pipe outside an engine, for benchmarks: the returned sink is what
/// the engine would dispatch into. Uploads go to the capturing client.
pub fn start_pipe_for_bench(
    config: &crate::telemetry::TelemetryConfig,
    host: super::TelemetryHost,
) -> (
    Arc<super::TelemetryPipe>,
    Arc<dyn crate::telemetry::TelemetrySink>,
) {
    let client = CapturingHttpClient::new();
    let pipe = super::TelemetryPipe::start(
        config,
        super::PipeParts {
            host,
            backend: super::store::Backend::Memory,
            device_id: None,
            client: Box::new(client),
            clock: Arc::new(|| chrono::Utc::now().timestamp_millis()),
            jitter: Box::new(|| 0),
            spawn_thread: true,
        },
    )
    .expect("a pipe starts");
    let sink = pipe.sink();
    (pipe, sink)
}
