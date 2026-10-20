package com.offlineprotocol.sdk;

public class MlsKeyPackageBundle {
    private java.lang.String packageId;
    private java.lang.String userId;
    private java.util.List<java.lang.Byte> keyPackageData;
    private long createdAtMs;
    private long expiresAtMs;
    private boolean synced;

    public MlsKeyPackageBundle(
        java.lang.String packageId, 
        java.lang.String userId, 
        java.util.List<java.lang.Byte> keyPackageData, 
        long createdAtMs, 
        long expiresAtMs, 
        boolean synced
    ) {
        
        this.packageId = packageId;
        
        this.userId = userId;
        
        this.keyPackageData = keyPackageData;
        
        this.createdAtMs = createdAtMs;
        
        this.expiresAtMs = expiresAtMs;
        
        this.synced = synced;
    }
    
    public java.lang.String packageId() {
        return this.packageId;
    }
    
    public java.lang.String userId() {
        return this.userId;
    }
    
    public java.util.List<java.lang.Byte> keyPackageData() {
        return this.keyPackageData;
    }
    
    public long createdAtMs() {
        return this.createdAtMs;
    }
    
    public long expiresAtMs() {
        return this.expiresAtMs;
    }
    
    public boolean synced() {
        return this.synced;
    }
    public void setPackageId(java.lang.String packageId) {
        this.packageId = packageId;
    }
    public void setUserId(java.lang.String userId) {
        this.userId = userId;
    }
    public void setKeyPackageData(java.util.List<java.lang.Byte> keyPackageData) {
        this.keyPackageData = keyPackageData;
    }
    public void setCreatedAtMs(long createdAtMs) {
        this.createdAtMs = createdAtMs;
    }
    public void setExpiresAtMs(long expiresAtMs) {
        this.expiresAtMs = expiresAtMs;
    }
    public void setSynced(boolean synced) {
        this.synced = synced;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MlsKeyPackageBundle) {
            MlsKeyPackageBundle _uniffiThat = (MlsKeyPackageBundle) _uniffiOther;
            return (
              java.util.Objects.equals(packageId, _uniffiThat.packageId) && 
              
              java.util.Objects.equals(userId, _uniffiThat.userId) && 
              
              java.util.Objects.equals(keyPackageData, _uniffiThat.keyPackageData) && 
              
              createdAtMs == _uniffiThat.createdAtMs && 
              
              expiresAtMs == _uniffiThat.expiresAtMs && 
              
              synced == _uniffiThat.synced
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(packageId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(userId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(keyPackageData);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(createdAtMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(expiresAtMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(synced);
        return _uniffiHash;
    }

    
    
    

}


