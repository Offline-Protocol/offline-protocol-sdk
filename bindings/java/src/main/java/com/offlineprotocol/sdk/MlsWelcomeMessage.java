package com.offlineprotocol.sdk;

public class MlsWelcomeMessage {
    private java.lang.String groupId;
    private java.util.List<java.lang.Byte> welcomeData;
    private java.lang.String inviterId;
    private java.lang.String groupName;
    private long timestampMs;

    public MlsWelcomeMessage(
        java.lang.String groupId, 
        java.util.List<java.lang.Byte> welcomeData, 
        java.lang.String inviterId, 
        java.lang.String groupName, 
        long timestampMs
    ) {
        
        this.groupId = groupId;
        
        this.welcomeData = welcomeData;
        
        this.inviterId = inviterId;
        
        this.groupName = groupName;
        
        this.timestampMs = timestampMs;
    }
    
    public java.lang.String groupId() {
        return this.groupId;
    }
    
    public java.util.List<java.lang.Byte> welcomeData() {
        return this.welcomeData;
    }
    
    public java.lang.String inviterId() {
        return this.inviterId;
    }
    
    public java.lang.String groupName() {
        return this.groupName;
    }
    
    public long timestampMs() {
        return this.timestampMs;
    }
    public void setGroupId(java.lang.String groupId) {
        this.groupId = groupId;
    }
    public void setWelcomeData(java.util.List<java.lang.Byte> welcomeData) {
        this.welcomeData = welcomeData;
    }
    public void setInviterId(java.lang.String inviterId) {
        this.inviterId = inviterId;
    }
    public void setGroupName(java.lang.String groupName) {
        this.groupName = groupName;
    }
    public void setTimestampMs(long timestampMs) {
        this.timestampMs = timestampMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MlsWelcomeMessage) {
            MlsWelcomeMessage _uniffiThat = (MlsWelcomeMessage) _uniffiOther;
            return (
              java.util.Objects.equals(groupId, _uniffiThat.groupId) && 
              
              java.util.Objects.equals(welcomeData, _uniffiThat.welcomeData) && 
              
              java.util.Objects.equals(inviterId, _uniffiThat.inviterId) && 
              
              java.util.Objects.equals(groupName, _uniffiThat.groupName) && 
              
              timestampMs == _uniffiThat.timestampMs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(groupId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(welcomeData);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(inviterId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(groupName);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(timestampMs);
        return _uniffiHash;
    }

    
    
    

}


