package com.offlineprotocol.sdk;

public class ReplyContext {
    private java.lang.String sender;
    private java.lang.String text;
    private java.lang.Long timestamp;
    private java.lang.String replyMediaLabel;
    private java.lang.String replyContentType;

    public ReplyContext(
        java.lang.String sender, 
        java.lang.String text, 
        java.lang.Long timestamp, 
        java.lang.String replyMediaLabel, 
        java.lang.String replyContentType
    ) {
        
        this.sender = sender;
        
        this.text = text;
        
        this.timestamp = timestamp;
        
        this.replyMediaLabel = replyMediaLabel;
        
        this.replyContentType = replyContentType;
    }
    
    public java.lang.String sender() {
        return this.sender;
    }
    
    public java.lang.String text() {
        return this.text;
    }
    
    public java.lang.Long timestamp() {
        return this.timestamp;
    }
    
    public java.lang.String replyMediaLabel() {
        return this.replyMediaLabel;
    }
    
    public java.lang.String replyContentType() {
        return this.replyContentType;
    }
    public void setSender(java.lang.String sender) {
        this.sender = sender;
    }
    public void setText(java.lang.String text) {
        this.text = text;
    }
    public void setTimestamp(java.lang.Long timestamp) {
        this.timestamp = timestamp;
    }
    public void setReplyMediaLabel(java.lang.String replyMediaLabel) {
        this.replyMediaLabel = replyMediaLabel;
    }
    public void setReplyContentType(java.lang.String replyContentType) {
        this.replyContentType = replyContentType;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ReplyContext) {
            ReplyContext _uniffiThat = (ReplyContext) _uniffiOther;
            return (
              java.util.Objects.equals(sender, _uniffiThat.sender) && 
              
              java.util.Objects.equals(text, _uniffiThat.text) && 
              
              java.util.Objects.equals(timestamp, _uniffiThat.timestamp) && 
              
              java.util.Objects.equals(replyMediaLabel, _uniffiThat.replyMediaLabel) && 
              
              java.util.Objects.equals(replyContentType, _uniffiThat.replyContentType)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(sender);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(text);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(timestamp);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyMediaLabel);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyContentType);
        return _uniffiHash;
    }

    
    
    

}


