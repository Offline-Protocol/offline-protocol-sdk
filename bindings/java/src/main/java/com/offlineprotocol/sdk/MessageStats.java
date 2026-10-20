package com.offlineprotocol.sdk;

public class MessageStats {
    private java.lang.String messageId;
    private long sentAtMs;
    private java.lang.Long deliveredAtMs;
    private byte hopCount;
    private java.lang.String status;

    public MessageStats(
        java.lang.String messageId, 
        long sentAtMs, 
        java.lang.Long deliveredAtMs, 
        byte hopCount, 
        java.lang.String status
    ) {
        
        this.messageId = messageId;
        
        this.sentAtMs = sentAtMs;
        
        this.deliveredAtMs = deliveredAtMs;
        
        this.hopCount = hopCount;
        
        this.status = status;
    }
    
    public java.lang.String messageId() {
        return this.messageId;
    }
    
    public long sentAtMs() {
        return this.sentAtMs;
    }
    
    public java.lang.Long deliveredAtMs() {
        return this.deliveredAtMs;
    }
    
    public byte hopCount() {
        return this.hopCount;
    }
    
    public java.lang.String status() {
        return this.status;
    }
    public void setMessageId(java.lang.String messageId) {
        this.messageId = messageId;
    }
    public void setSentAtMs(long sentAtMs) {
        this.sentAtMs = sentAtMs;
    }
    public void setDeliveredAtMs(java.lang.Long deliveredAtMs) {
        this.deliveredAtMs = deliveredAtMs;
    }
    public void setHopCount(byte hopCount) {
        this.hopCount = hopCount;
    }
    public void setStatus(java.lang.String status) {
        this.status = status;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MessageStats) {
            MessageStats _uniffiThat = (MessageStats) _uniffiOther;
            return (
              java.util.Objects.equals(messageId, _uniffiThat.messageId) && 
              
              sentAtMs == _uniffiThat.sentAtMs && 
              
              java.util.Objects.equals(deliveredAtMs, _uniffiThat.deliveredAtMs) && 
              
              hopCount == _uniffiThat.hopCount && 
              
              java.util.Objects.equals(status, _uniffiThat.status)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(messageId);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(sentAtMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(deliveredAtMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(hopCount);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(status);
        return _uniffiHash;
    }

    
    
    

}


