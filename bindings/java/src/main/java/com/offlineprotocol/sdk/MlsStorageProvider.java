package com.offlineprotocol.sdk;

public interface MlsStorageProvider {
    public void store(java.lang.String keyType, java.lang.String keyId, java.util.List<java.lang.Byte> data) throws MlsStorageException;
    public java.util.List<java.lang.Byte> load(java.lang.String keyType, java.lang.String keyId) throws MlsStorageException;
    public void delete(java.lang.String keyType, java.lang.String keyId) throws MlsStorageException;
    public java.util.List<java.lang.String> listKeys(java.lang.String keyType) throws MlsStorageException;
    
}
