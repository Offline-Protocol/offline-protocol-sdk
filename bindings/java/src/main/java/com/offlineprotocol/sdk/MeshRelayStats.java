package com.offlineprotocol.sdk;

public class MeshRelayStats {
    private long forwarded;
    private long transmissions;
    private long queued;
    private long awaitingTransmission;
    private long duplicatesSuppressed;
    private long coveredByANeighbor;
    private long peerRateLimited;
    private long refusedQueueFull;
    private long rateDeferred;
    private long abandonedOverdue;
    private long hopLimitReached;
    private long reachClamped;
    private long droppedForCapacity;

    public MeshRelayStats(
        long forwarded, 
        long transmissions, 
        long queued, 
        long awaitingTransmission, 
        long duplicatesSuppressed, 
        long coveredByANeighbor, 
        long peerRateLimited, 
        long refusedQueueFull, 
        long rateDeferred, 
        long abandonedOverdue, 
        long hopLimitReached, 
        long reachClamped, 
        long droppedForCapacity
    ) {
        
        this.forwarded = forwarded;
        
        this.transmissions = transmissions;
        
        this.queued = queued;
        
        this.awaitingTransmission = awaitingTransmission;
        
        this.duplicatesSuppressed = duplicatesSuppressed;
        
        this.coveredByANeighbor = coveredByANeighbor;
        
        this.peerRateLimited = peerRateLimited;
        
        this.refusedQueueFull = refusedQueueFull;
        
        this.rateDeferred = rateDeferred;
        
        this.abandonedOverdue = abandonedOverdue;
        
        this.hopLimitReached = hopLimitReached;
        
        this.reachClamped = reachClamped;
        
        this.droppedForCapacity = droppedForCapacity;
    }
    
    public long forwarded() {
        return this.forwarded;
    }
    
    public long transmissions() {
        return this.transmissions;
    }
    
    public long queued() {
        return this.queued;
    }
    
    public long awaitingTransmission() {
        return this.awaitingTransmission;
    }
    
    public long duplicatesSuppressed() {
        return this.duplicatesSuppressed;
    }
    
    public long coveredByANeighbor() {
        return this.coveredByANeighbor;
    }
    
    public long peerRateLimited() {
        return this.peerRateLimited;
    }
    
    public long refusedQueueFull() {
        return this.refusedQueueFull;
    }
    
    public long rateDeferred() {
        return this.rateDeferred;
    }
    
    public long abandonedOverdue() {
        return this.abandonedOverdue;
    }
    
    public long hopLimitReached() {
        return this.hopLimitReached;
    }
    
    public long reachClamped() {
        return this.reachClamped;
    }
    
    public long droppedForCapacity() {
        return this.droppedForCapacity;
    }
    public void setForwarded(long forwarded) {
        this.forwarded = forwarded;
    }
    public void setTransmissions(long transmissions) {
        this.transmissions = transmissions;
    }
    public void setQueued(long queued) {
        this.queued = queued;
    }
    public void setAwaitingTransmission(long awaitingTransmission) {
        this.awaitingTransmission = awaitingTransmission;
    }
    public void setDuplicatesSuppressed(long duplicatesSuppressed) {
        this.duplicatesSuppressed = duplicatesSuppressed;
    }
    public void setCoveredByANeighbor(long coveredByANeighbor) {
        this.coveredByANeighbor = coveredByANeighbor;
    }
    public void setPeerRateLimited(long peerRateLimited) {
        this.peerRateLimited = peerRateLimited;
    }
    public void setRefusedQueueFull(long refusedQueueFull) {
        this.refusedQueueFull = refusedQueueFull;
    }
    public void setRateDeferred(long rateDeferred) {
        this.rateDeferred = rateDeferred;
    }
    public void setAbandonedOverdue(long abandonedOverdue) {
        this.abandonedOverdue = abandonedOverdue;
    }
    public void setHopLimitReached(long hopLimitReached) {
        this.hopLimitReached = hopLimitReached;
    }
    public void setReachClamped(long reachClamped) {
        this.reachClamped = reachClamped;
    }
    public void setDroppedForCapacity(long droppedForCapacity) {
        this.droppedForCapacity = droppedForCapacity;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MeshRelayStats) {
            MeshRelayStats _uniffiThat = (MeshRelayStats) _uniffiOther;
            return (
              forwarded == _uniffiThat.forwarded && 
              
              transmissions == _uniffiThat.transmissions && 
              
              queued == _uniffiThat.queued && 
              
              awaitingTransmission == _uniffiThat.awaitingTransmission && 
              
              duplicatesSuppressed == _uniffiThat.duplicatesSuppressed && 
              
              coveredByANeighbor == _uniffiThat.coveredByANeighbor && 
              
              peerRateLimited == _uniffiThat.peerRateLimited && 
              
              refusedQueueFull == _uniffiThat.refusedQueueFull && 
              
              rateDeferred == _uniffiThat.rateDeferred && 
              
              abandonedOverdue == _uniffiThat.abandonedOverdue && 
              
              hopLimitReached == _uniffiThat.hopLimitReached && 
              
              reachClamped == _uniffiThat.reachClamped && 
              
              droppedForCapacity == _uniffiThat.droppedForCapacity
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(forwarded);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(transmissions);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(queued);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(awaitingTransmission);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(duplicatesSuppressed);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(coveredByANeighbor);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(peerRateLimited);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedQueueFull);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(rateDeferred);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(abandonedOverdue);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(hopLimitReached);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(reachClamped);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(droppedForCapacity);
        return _uniffiHash;
    }

    
    
    

}


