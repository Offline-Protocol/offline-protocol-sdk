package com.offlineprotocol.sdk;

public class ReticulumMessage {
    private java.lang.String messageId;
    private java.lang.String recipientId;
    private java.util.List<java.lang.Byte> data;
    private java.lang.String replyToMsg;

    public ReticulumMessage(
        java.lang.String messageId, 
        java.lang.String recipientId, 
        java.util.List<java.lang.Byte> data, 
        java.lang.String replyToMsg
    ) {
        
        this.messageId = messageId;
        
        this.recipientId = recipientId;
        
        this.data = data;
        
        this.replyToMsg = replyToMsg;
    }
    
    public java.lang.String messageId() {
        return this.messageId;
    }
    
    public java.lang.String recipientId() {
        return this.recipientId;
    }
    
    public java.util.List<java.lang.Byte> data() {
        return this.data;
    }
    
    public java.lang.String replyToMsg() {
        return this.replyToMsg;
    }
    public void setMessageId(java.lang.String messageId) {
        this.messageId = messageId;
    }
    public void setRecipientId(java.lang.String recipientId) {
        this.recipientId = recipientId;
    }
    public void setData(java.util.List<java.lang.Byte> data) {
        this.data = data;
    }
    public void setReplyToMsg(java.lang.String replyToMsg) {
        this.replyToMsg = replyToMsg;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ReticulumMessage) {
            ReticulumMessage _uniffiThat = (ReticulumMessage) _uniffiOther;
            return (
              java.util.Objects.equals(messageId, _uniffiThat.messageId) && 
              
              java.util.Objects.equals(recipientId, _uniffiThat.recipientId) && 
              
              java.util.Objects.equals(data, _uniffiThat.data) && 
              
              java.util.Objects.equals(replyToMsg, _uniffiThat.replyToMsg)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(messageId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(recipientId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(data);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyToMsg);
        return _uniffiHash;
    }

    
    
    

}


