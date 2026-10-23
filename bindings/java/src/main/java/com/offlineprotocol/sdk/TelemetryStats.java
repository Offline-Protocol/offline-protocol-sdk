package com.offlineprotocol.sdk;

public class TelemetryStats {
    private long buffered;
    private long sentEvents;
    private long acceptedEvents;
    private long dropped;
    private java.lang.String sessionId;
    private java.lang.String lastError;
    private java.lang.Long lastFlushAtMs;

    public TelemetryStats(
        long buffered, 
        long sentEvents, 
        long acceptedEvents, 
        long dropped, 
        java.lang.String sessionId, 
        java.lang.String lastError, 
        java.lang.Long lastFlushAtMs
    ) {
        
        this.buffered = buffered;
        
        this.sentEvents = sentEvents;
        
        this.acceptedEvents = acceptedEvents;
        
        this.dropped = dropped;
        
        this.sessionId = sessionId;
        
        this.lastError = lastError;
        
        this.lastFlushAtMs = lastFlushAtMs;
    }
    
    public long buffered() {
        return this.buffered;
    }
    
    public long sentEvents() {
        return this.sentEvents;
    }
    
    public long acceptedEvents() {
        return this.acceptedEvents;
    }
    
    public long dropped() {
        return this.dropped;
    }
    
    public java.lang.String sessionId() {
        return this.sessionId;
    }
    
    public java.lang.String lastError() {
        return this.lastError;
    }
    
    public java.lang.Long lastFlushAtMs() {
        return this.lastFlushAtMs;
    }
    public void setBuffered(long buffered) {
        this.buffered = buffered;
    }
    public void setSentEvents(long sentEvents) {
        this.sentEvents = sentEvents;
    }
    public void setAcceptedEvents(long acceptedEvents) {
        this.acceptedEvents = acceptedEvents;
    }
    public void setDropped(long dropped) {
        this.dropped = dropped;
    }
    public void setSessionId(java.lang.String sessionId) {
        this.sessionId = sessionId;
    }
    public void setLastError(java.lang.String lastError) {
        this.lastError = lastError;
    }
    public void setLastFlushAtMs(java.lang.Long lastFlushAtMs) {
        this.lastFlushAtMs = lastFlushAtMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof TelemetryStats) {
            TelemetryStats _uniffiThat = (TelemetryStats) _uniffiOther;
            return (
              buffered == _uniffiThat.buffered && 
              
              sentEvents == _uniffiThat.sentEvents && 
              
              acceptedEvents == _uniffiThat.acceptedEvents && 
              
              dropped == _uniffiThat.dropped && 
              
              java.util.Objects.equals(sessionId, _uniffiThat.sessionId) && 
              
              java.util.Objects.equals(lastError, _uniffiThat.lastError) && 
              
              java.util.Objects.equals(lastFlushAtMs, _uniffiThat.lastFlushAtMs)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(buffered);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(sentEvents);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(acceptedEvents);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(dropped);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(sessionId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(lastError);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(lastFlushAtMs);
        return _uniffiHash;
    }

    
    
    

}


