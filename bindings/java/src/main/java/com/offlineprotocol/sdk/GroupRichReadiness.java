package com.offlineprotocol.sdk;

public class GroupRichReadiness {
    private boolean ready;
    private java.util.List<java.lang.String> unknownMembers;

    public GroupRichReadiness(
        boolean ready, 
        java.util.List<java.lang.String> unknownMembers
    ) {
        
        this.ready = ready;
        
        this.unknownMembers = unknownMembers;
    }
    
    public boolean ready() {
        return this.ready;
    }
    
    public java.util.List<java.lang.String> unknownMembers() {
        return this.unknownMembers;
    }
    public void setReady(boolean ready) {
        this.ready = ready;
    }
    public void setUnknownMembers(java.util.List<java.lang.String> unknownMembers) {
        this.unknownMembers = unknownMembers;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof GroupRichReadiness) {
            GroupRichReadiness _uniffiThat = (GroupRichReadiness) _uniffiOther;
            return (
              ready == _uniffiThat.ready && 
              
              java.util.Objects.equals(unknownMembers, _uniffiThat.unknownMembers)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(ready);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(unknownMembers);
        return _uniffiHash;
    }

    
    
    

}


