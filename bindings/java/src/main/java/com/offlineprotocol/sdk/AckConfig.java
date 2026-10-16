package com.offlineprotocol.sdk;

public class AckConfig {
    private long defaultTimeoutMs;
    private long maxPendingAcks;

    public AckConfig(
        long defaultTimeoutMs, 
        long maxPendingAcks
    ) {
        
        this.defaultTimeoutMs = defaultTimeoutMs;
        
        this.maxPendingAcks = maxPendingAcks;
    }
    
    public long defaultTimeoutMs() {
        return this.defaultTimeoutMs;
    }
    
    public long maxPendingAcks() {
        return this.maxPendingAcks;
    }
    public void setDefaultTimeoutMs(long defaultTimeoutMs) {
        this.defaultTimeoutMs = defaultTimeoutMs;
    }
    public void setMaxPendingAcks(long maxPendingAcks) {
        this.maxPendingAcks = maxPendingAcks;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof AckConfig) {
            AckConfig _uniffiThat = (AckConfig) _uniffiOther;
            return (
              defaultTimeoutMs == _uniffiThat.defaultTimeoutMs && 
              
              maxPendingAcks == _uniffiThat.maxPendingAcks
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(defaultTimeoutMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxPendingAcks);
        return _uniffiHash;
    }

    
    
    

}


