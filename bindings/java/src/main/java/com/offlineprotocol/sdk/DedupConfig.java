package com.offlineprotocol.sdk;

public class DedupConfig {
    private long maxTrackedMessages;
    private long retentionTimeSecs;

    public DedupConfig(
        long maxTrackedMessages, 
        long retentionTimeSecs
    ) {
        
        this.maxTrackedMessages = maxTrackedMessages;
        
        this.retentionTimeSecs = retentionTimeSecs;
    }
    
    public long maxTrackedMessages() {
        return this.maxTrackedMessages;
    }
    
    public long retentionTimeSecs() {
        return this.retentionTimeSecs;
    }
    public void setMaxTrackedMessages(long maxTrackedMessages) {
        this.maxTrackedMessages = maxTrackedMessages;
    }
    public void setRetentionTimeSecs(long retentionTimeSecs) {
        this.retentionTimeSecs = retentionTimeSecs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof DedupConfig) {
            DedupConfig _uniffiThat = (DedupConfig) _uniffiOther;
            return (
              maxTrackedMessages == _uniffiThat.maxTrackedMessages && 
              
              retentionTimeSecs == _uniffiThat.retentionTimeSecs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxTrackedMessages);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(retentionTimeSecs);
        return _uniffiHash;
    }

    
    
    

}


