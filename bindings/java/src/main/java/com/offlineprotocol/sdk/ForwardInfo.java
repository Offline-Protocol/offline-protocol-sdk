package com.offlineprotocol.sdk;

public class ForwardInfo {
    private java.lang.String originalSender;
    private java.lang.String originalMessageId;
    private long originalTimestamp;
    private int forwardCount;

    public ForwardInfo(
        java.lang.String originalSender, 
        java.lang.String originalMessageId, 
        long originalTimestamp, 
        int forwardCount
    ) {
        
        this.originalSender = originalSender;
        
        this.originalMessageId = originalMessageId;
        
        this.originalTimestamp = originalTimestamp;
        
        this.forwardCount = forwardCount;
    }
    
    public java.lang.String originalSender() {
        return this.originalSender;
    }
    
    public java.lang.String originalMessageId() {
        return this.originalMessageId;
    }
    
    public long originalTimestamp() {
        return this.originalTimestamp;
    }
    
    public int forwardCount() {
        return this.forwardCount;
    }
    public void setOriginalSender(java.lang.String originalSender) {
        this.originalSender = originalSender;
    }
    public void setOriginalMessageId(java.lang.String originalMessageId) {
        this.originalMessageId = originalMessageId;
    }
    public void setOriginalTimestamp(long originalTimestamp) {
        this.originalTimestamp = originalTimestamp;
    }
    public void setForwardCount(int forwardCount) {
        this.forwardCount = forwardCount;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ForwardInfo) {
            ForwardInfo _uniffiThat = (ForwardInfo) _uniffiOther;
            return (
              java.util.Objects.equals(originalSender, _uniffiThat.originalSender) && 
              
              java.util.Objects.equals(originalMessageId, _uniffiThat.originalMessageId) && 
              
              originalTimestamp == _uniffiThat.originalTimestamp && 
              
              forwardCount == _uniffiThat.forwardCount
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(originalSender);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(originalMessageId);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(originalTimestamp);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(forwardCount);
        return _uniffiHash;
    }

    
    
    

}


