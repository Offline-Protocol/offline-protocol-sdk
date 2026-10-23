package com.offlineprotocol.sdk;

public interface ProtocolStateStorageProvider {
    public void store(java.lang.String keyType, java.lang.String keyId, byte[] data) throws MlsStorageException;
    public byte[] load(java.lang.String keyType, java.lang.String keyId) throws MlsStorageException;
    public void delete(java.lang.String keyType, java.lang.String keyId) throws MlsStorageException;
    public java.util.List<java.lang.String> listKeys(java.lang.String keyType) throws MlsStorageException;
    
}
