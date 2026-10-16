package com.offlineprotocol.sdk;

public class NostrMessage {
    private java.lang.String messageId;
    private java.lang.String eventId;
    private java.lang.String eventJson;

    public NostrMessage(
        java.lang.String messageId, 
        java.lang.String eventId, 
        java.lang.String eventJson
    ) {
        
        this.messageId = messageId;
        
        this.eventId = eventId;
        
        this.eventJson = eventJson;
    }
    
    public java.lang.String messageId() {
        return this.messageId;
    }
    
    public java.lang.String eventId() {
        return this.eventId;
    }
    
    public java.lang.String eventJson() {
        return this.eventJson;
    }
    public void setMessageId(java.lang.String messageId) {
        this.messageId = messageId;
    }
    public void setEventId(java.lang.String eventId) {
        this.eventId = eventId;
    }
    public void setEventJson(java.lang.String eventJson) {
        this.eventJson = eventJson;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof NostrMessage) {
            NostrMessage _uniffiThat = (NostrMessage) _uniffiOther;
            return (
              java.util.Objects.equals(messageId, _uniffiThat.messageId) && 
              
              java.util.Objects.equals(eventId, _uniffiThat.eventId) && 
              
              java.util.Objects.equals(eventJson, _uniffiThat.eventJson)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(messageId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(eventId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(eventJson);
        return _uniffiHash;
    }

    
    
    

}


