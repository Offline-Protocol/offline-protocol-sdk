package com.offlineprotocol.sdk;

public class MlsGroupInfo {
    private java.lang.String groupId;
    private java.lang.String name;
    private java.util.List<java.lang.String> members;
    private long epoch;
    private boolean isSession;
    private long createdAtMs;
    private long lastActivityMs;

    public MlsGroupInfo(
        java.lang.String groupId, 
        java.lang.String name, 
        java.util.List<java.lang.String> members, 
        long epoch, 
        boolean isSession, 
        long createdAtMs, 
        long lastActivityMs
    ) {
        
        this.groupId = groupId;
        
        this.name = name;
        
        this.members = members;
        
        this.epoch = epoch;
        
        this.isSession = isSession;
        
        this.createdAtMs = createdAtMs;
        
        this.lastActivityMs = lastActivityMs;
    }
    
    public java.lang.String groupId() {
        return this.groupId;
    }
    
    public java.lang.String name() {
        return this.name;
    }
    
    public java.util.List<java.lang.String> members() {
        return this.members;
    }
    
    public long epoch() {
        return this.epoch;
    }
    
    public boolean isSession() {
        return this.isSession;
    }
    
    public long createdAtMs() {
        return this.createdAtMs;
    }
    
    public long lastActivityMs() {
        return this.lastActivityMs;
    }
    public void setGroupId(java.lang.String groupId) {
        this.groupId = groupId;
    }
    public void setName(java.lang.String name) {
        this.name = name;
    }
    public void setMembers(java.util.List<java.lang.String> members) {
        this.members = members;
    }
    public void setEpoch(long epoch) {
        this.epoch = epoch;
    }
    public void setIsSession(boolean isSession) {
        this.isSession = isSession;
    }
    public void setCreatedAtMs(long createdAtMs) {
        this.createdAtMs = createdAtMs;
    }
    public void setLastActivityMs(long lastActivityMs) {
        this.lastActivityMs = lastActivityMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MlsGroupInfo) {
            MlsGroupInfo _uniffiThat = (MlsGroupInfo) _uniffiOther;
            return (
              java.util.Objects.equals(groupId, _uniffiThat.groupId) && 
              
              java.util.Objects.equals(name, _uniffiThat.name) && 
              
              java.util.Objects.equals(members, _uniffiThat.members) && 
              
              epoch == _uniffiThat.epoch && 
              
              isSession == _uniffiThat.isSession && 
              
              createdAtMs == _uniffiThat.createdAtMs && 
              
              lastActivityMs == _uniffiThat.lastActivityMs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(groupId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(name);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(members);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(epoch);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(isSession);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(createdAtMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(lastActivityMs);
        return _uniffiHash;
    }

    
    
    

}


