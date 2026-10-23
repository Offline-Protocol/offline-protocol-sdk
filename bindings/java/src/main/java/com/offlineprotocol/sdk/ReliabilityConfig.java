package com.offlineprotocol.sdk;

public class ReliabilityConfig {
    private AckConfig ack;
    private RetryConfig retry;
    private DedupConfig dedup;

    public ReliabilityConfig(
        AckConfig ack, 
        RetryConfig retry, 
        DedupConfig dedup
    ) {
        
        this.ack = ack;
        
        this.retry = retry;
        
        this.dedup = dedup;
    }
    
    public AckConfig ack() {
        return this.ack;
    }
    
    public RetryConfig retry() {
        return this.retry;
    }
    
    public DedupConfig dedup() {
        return this.dedup;
    }
    public void setAck(AckConfig ack) {
        this.ack = ack;
    }
    public void setRetry(RetryConfig retry) {
        this.retry = retry;
    }
    public void setDedup(DedupConfig dedup) {
        this.dedup = dedup;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ReliabilityConfig) {
            ReliabilityConfig _uniffiThat = (ReliabilityConfig) _uniffiOther;
            return (
              java.util.Objects.equals(ack, _uniffiThat.ack) && 
              
              java.util.Objects.equals(retry, _uniffiThat.retry) && 
              
              java.util.Objects.equals(dedup, _uniffiThat.dedup)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(ack);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(retry);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(dedup);
        return _uniffiHash;
    }

    
    
    

}


