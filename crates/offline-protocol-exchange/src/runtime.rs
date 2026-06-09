//! Adapter runtime gate.
//!
//! Loading an adapter is loading third-party behavior. The runtime trait is
//! the single choke point: the exchange only calls [`AdapterRuntime::load`]
//! after signature and content-hash verification both pass, and runtimes must
//! treat any other entry path as unloadable. The real runtime (e.g. a
//! llama.cpp LoRA loader in the host app) implements this trait; CI uses
//! [`StubAdapterRuntime`].

use crate::error::{ExchangeError, ExchangeResult};
use crate::types::Listing;
use std::sync::Mutex;

/// Loads verified adapter artifacts into the local model runtime.
pub trait AdapterRuntime: Send + Sync {
    /// Loads the adapter at `artifact_path`. Called only after the exchange
    /// has verified the listing attestation and the artifact content hash.
    fn load(&self, listing: &Listing, artifact_path: &str) -> ExchangeResult<()>;
}

/// Records loads without touching a real model runtime. Used in CI and as a
/// placeholder until a host wires a real runtime.
#[derive(Default)]
pub struct StubAdapterRuntime {
    loaded: Mutex<Vec<(String, String)>>,
}

impl StubAdapterRuntime {
    /// Creates an empty stub runtime.
    pub fn new() -> Self {
        Self::default()
    }

    /// `(service_id, artifact_path)` pairs loaded so far.
    pub fn loaded(&self) -> Vec<(String, String)> {
        self.loaded.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

impl AdapterRuntime for StubAdapterRuntime {
    fn load(&self, listing: &Listing, artifact_path: &str) -> ExchangeResult<()> {
        self.loaded
            .lock()
            .map_err(|_| ExchangeError::Serialization("stub runtime lock poisoned".into()))?
            .push((listing.service_id().to_string(), artifact_path.to_string()));
        Ok(())
    }
}
