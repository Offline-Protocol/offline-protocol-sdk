package com.offlineprotocol.sdk;

public class ServicePublishReport {
    private int subscribers;
    private int sentLive;
    private int queued;
    private int coalesced;
    private int failed;

    public ServicePublishReport(
        int subscribers, 
        int sentLive, 
        int queued, 
        int coalesced, 
        int failed
    ) {
        
        this.subscribers = subscribers;
        
        this.sentLive = sentLive;
        
        this.queued = queued;
        
        this.coalesced = coalesced;
        
        this.failed = failed;
    }
    
    public int subscribers() {
        return this.subscribers;
    }
    
    public int sentLive() {
        return this.sentLive;
    }
    
    public int queued() {
        return this.queued;
    }
    
    public int coalesced() {
        return this.coalesced;
    }
    
    public int failed() {
        return this.failed;
    }
    public void setSubscribers(int subscribers) {
        this.subscribers = subscribers;
    }
    public void setSentLive(int sentLive) {
        this.sentLive = sentLive;
    }
    public void setQueued(int queued) {
        this.queued = queued;
    }
    public void setCoalesced(int coalesced) {
        this.coalesced = coalesced;
    }
    public void setFailed(int failed) {
        this.failed = failed;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ServicePublishReport) {
            ServicePublishReport _uniffiThat = (ServicePublishReport) _uniffiOther;
            return (
              subscribers == _uniffiThat.subscribers && 
              
              sentLive == _uniffiThat.sentLive && 
              
              queued == _uniffiThat.queued && 
              
              coalesced == _uniffiThat.coalesced && 
              
              failed == _uniffiThat.failed
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(subscribers);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(sentLive);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(queued);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(coalesced);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(failed);
        return _uniffiHash;
    }

    
    
    

}


