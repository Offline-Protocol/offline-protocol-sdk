package com.offlineprotocol.sdk;

public class MlsEncryptedMessage {
    private java.lang.String groupId;
    private java.lang.String messageType;
    private long epoch;
    private java.util.List<java.lang.Byte> ciphertext;
    private java.lang.String senderId;
    private long timestampMs;

    public MlsEncryptedMessage(
        java.lang.String groupId, 
        java.lang.String messageType, 
        long epoch, 
        java.util.List<java.lang.Byte> ciphertext, 
        java.lang.String senderId, 
        long timestampMs
    ) {
        
        this.groupId = groupId;
        
        this.messageType = messageType;
        
        this.epoch = epoch;
        
        this.ciphertext = ciphertext;
        
        this.senderId = senderId;
        
        this.timestampMs = timestampMs;
    }
    
    public java.lang.String groupId() {
        return this.groupId;
    }
    
    public java.lang.String messageType() {
        return this.messageType;
    }
    
    public long epoch() {
        return this.epoch;
    }
    
    public java.util.List<java.lang.Byte> ciphertext() {
        return this.ciphertext;
    }
    
    public java.lang.String senderId() {
        return this.senderId;
    }
    
    public long timestampMs() {
        return this.timestampMs;
    }
    public void setGroupId(java.lang.String groupId) {
        this.groupId = groupId;
    }
    public void setMessageType(java.lang.String messageType) {
        this.messageType = messageType;
    }
    public void setEpoch(long epoch) {
        this.epoch = epoch;
    }
    public void setCiphertext(java.util.List<java.lang.Byte> ciphertext) {
        this.ciphertext = ciphertext;
    }
    public void setSenderId(java.lang.String senderId) {
        this.senderId = senderId;
    }
    public void setTimestampMs(long timestampMs) {
        this.timestampMs = timestampMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MlsEncryptedMessage) {
            MlsEncryptedMessage _uniffiThat = (MlsEncryptedMessage) _uniffiOther;
            return (
              java.util.Objects.equals(groupId, _uniffiThat.groupId) && 
              
              java.util.Objects.equals(messageType, _uniffiThat.messageType) && 
              
              epoch == _uniffiThat.epoch && 
              
              java.util.Objects.equals(ciphertext, _uniffiThat.ciphertext) && 
              
              java.util.Objects.equals(senderId, _uniffiThat.senderId) && 
              
              timestampMs == _uniffiThat.timestampMs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(groupId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(messageType);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(epoch);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(ciphertext);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(senderId);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(timestampMs);
        return _uniffiHash;
    }

    
    
    

}


