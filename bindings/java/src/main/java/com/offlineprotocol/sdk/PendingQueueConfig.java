package com.offlineprotocol.sdk;

public class PendingQueueConfig {
    private long maxPendingPerPeer;
    private long maxPendingGlobal;
    private long pendingTtlMs;
    private OverflowPolicy overflowPolicy;

    public PendingQueueConfig(
        long maxPendingPerPeer, 
        long maxPendingGlobal, 
        long pendingTtlMs, 
        OverflowPolicy overflowPolicy
    ) {
        
        this.maxPendingPerPeer = maxPendingPerPeer;
        
        this.maxPendingGlobal = maxPendingGlobal;
        
        this.pendingTtlMs = pendingTtlMs;
        
        this.overflowPolicy = overflowPolicy;
    }
    
    public long maxPendingPerPeer() {
        return this.maxPendingPerPeer;
    }
    
    public long maxPendingGlobal() {
        return this.maxPendingGlobal;
    }
    
    public long pendingTtlMs() {
        return this.pendingTtlMs;
    }
    
    public OverflowPolicy overflowPolicy() {
        return this.overflowPolicy;
    }
    public void setMaxPendingPerPeer(long maxPendingPerPeer) {
        this.maxPendingPerPeer = maxPendingPerPeer;
    }
    public void setMaxPendingGlobal(long maxPendingGlobal) {
        this.maxPendingGlobal = maxPendingGlobal;
    }
    public void setPendingTtlMs(long pendingTtlMs) {
        this.pendingTtlMs = pendingTtlMs;
    }
    public void setOverflowPolicy(OverflowPolicy overflowPolicy) {
        this.overflowPolicy = overflowPolicy;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof PendingQueueConfig) {
            PendingQueueConfig _uniffiThat = (PendingQueueConfig) _uniffiOther;
            return (
              maxPendingPerPeer == _uniffiThat.maxPendingPerPeer && 
              
              maxPendingGlobal == _uniffiThat.maxPendingGlobal && 
              
              pendingTtlMs == _uniffiThat.pendingTtlMs && 
              
              java.util.Objects.equals(overflowPolicy, _uniffiThat.overflowPolicy)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxPendingPerPeer);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxPendingGlobal);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(pendingTtlMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(overflowPolicy);
        return _uniffiHash;
    }

    
    
    

}


