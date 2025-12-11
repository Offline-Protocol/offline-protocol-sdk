use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider;
use crate::storage_adapter::MlsStorageAdapter;
use std::sync::Arc;

/// Custom OpenMLS provider that uses our MlsStorageAdapter for persistence.
#[derive(Clone)]
pub struct MlsProvider {
    inner: Arc<OpenMlsRustCrypto>,
    storage: MlsStorageAdapter,
}

impl MlsProvider {
    pub fn new(storage: MlsStorageAdapter) -> Self {
        Self {
            inner: Arc::new(OpenMlsRustCrypto::default()),
            storage,
        }
    }
}

impl OpenMlsProvider for MlsProvider {
    type CryptoProvider = <OpenMlsRustCrypto as OpenMlsProvider>::CryptoProvider;
    type RandProvider = <OpenMlsRustCrypto as OpenMlsProvider>::RandProvider;
    type StorageProvider = MlsStorageAdapter;

    fn crypto(&self) -> &Self::CryptoProvider {
        self.inner.crypto()
    }

    fn rand(&self) -> &Self::RandProvider {
        self.inner.rand()
    }

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }
}
