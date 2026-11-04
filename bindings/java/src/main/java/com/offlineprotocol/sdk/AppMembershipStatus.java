package com.offlineprotocol.sdk;

public class AppMembershipStatus {
    private boolean configured;
    private java.lang.Long certificateNotAfter;
    private int visibleMembers;

    public AppMembershipStatus(
        boolean configured, 
        java.lang.Long certificateNotAfter, 
        int visibleMembers
    ) {
        
        this.configured = configured;
        
        this.certificateNotAfter = certificateNotAfter;
        
        this.visibleMembers = visibleMembers;
    }
    
    public boolean configured() {
        return this.configured;
    }
    
    public java.lang.Long certificateNotAfter() {
        return this.certificateNotAfter;
    }
    
    public int visibleMembers() {
        return this.visibleMembers;
    }
    public void setConfigured(boolean configured) {
        this.configured = configured;
    }
    public void setCertificateNotAfter(java.lang.Long certificateNotAfter) {
        this.certificateNotAfter = certificateNotAfter;
    }
    public void setVisibleMembers(int visibleMembers) {
        this.visibleMembers = visibleMembers;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof AppMembershipStatus) {
            AppMembershipStatus _uniffiThat = (AppMembershipStatus) _uniffiOther;
            return (
              configured == _uniffiThat.configured && 
              
              java.util.Objects.equals(certificateNotAfter, _uniffiThat.certificateNotAfter) && 
              
              visibleMembers == _uniffiThat.visibleMembers
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(configured);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(certificateNotAfter);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(visibleMembers);
        return _uniffiHash;
    }

    
    
    

}


