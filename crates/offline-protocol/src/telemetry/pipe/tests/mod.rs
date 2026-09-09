//! Pipe tests that span modules: the boundary guard, the performance
//! proofs, and the end-to-end scenarios.

mod boundary;
mod golden;
mod performance;
mod scenarios;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::telemetry::pipe::host::{TelemetryHost, TelemetryOs};
use crate::telemetry::pipe::store::Backend;
use crate::telemetry::pipe::uploader::{HttpClient, Request, Response};
use crate::telemetry::pipe::{Clock, PipeParts, TelemetryPipe};
use crate::telemetry::TelemetryConfig;

/// A clock the test advances.
#[derive(Clone, Default)]
pub(crate) struct FakeClock(Arc<AtomicI64>);

impl FakeClock {
    pub(crate) fn at(ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(ms)))
    }

    pub(crate) fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub(crate) fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    pub(crate) fn clock(&self) -> Clock {
        let inner = self.0.clone();
        Arc::new(move || inner.load(Ordering::SeqCst))
    }
}

/// A client that captures every body and answers with a fixed status.
#[derive(Clone)]
pub(crate) struct CapturingClient {
    pub(crate) bodies: Arc<Mutex<Vec<String>>>,
    pub(crate) status: Arc<AtomicI64>,
}

impl CapturingClient {
    pub(crate) fn accepting() -> Self {
        Self {
            bodies: Arc::default(),
            status: Arc::new(AtomicI64::new(202)),
        }
    }

    pub(crate) fn batches(&self) -> Vec<serde_json::Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|b| serde_json::from_str(b).expect("body is JSON"))
            .collect()
    }
}

impl HttpClient for CapturingClient {
    fn post(&mut self, request: &Request<'_>) -> Result<Response, String> {
        let status = self.status.load(Ordering::SeqCst);
        if status <= 0 {
            return Err("connection refused".into());
        }
        self.bodies.lock().unwrap().push(request.body.to_string());
        let accepted = serde_json::from_str::<serde_json::Value>(request.body)
            .ok()
            .and_then(|v| v["events"].as_array().map(|e| e.len()))
            .unwrap_or(0);
        Ok(Response {
            status: status as u16,
            retry_after: None,
            body: format!(r#"{{"accepted":{accepted},"rejected":0,"rejected_reasons":[]}}"#),
        })
    }
}

pub(crate) fn test_config() -> TelemetryConfig {
    TelemetryConfig::default()
        .with_api_key("mp_test_key")
        .with_app_id("app_test")
        .with_app_version("1.4.2")
}

/// A pipe with no thread, driven by explicit cycles.
pub(crate) fn inline_pipe(
    config: &TelemetryConfig,
    clock: &FakeClock,
    client: CapturingClient,
    backend: Backend,
) -> Arc<TelemetryPipe> {
    TelemetryPipe::start(
        config,
        PipeParts {
            host: TelemetryHost::new(TelemetryOs::Ios, 18),
            backend,
            device_id: None,
            client: Box::new(client),
            clock: clock.clock(),
            jitter: Box::new(|| 0),
            spawn_thread: false,
        },
    )
    .expect("pipe starts")
}
