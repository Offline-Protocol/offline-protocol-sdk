package com.offlineprotocol.sdk;

public class TransportConfig {
    private boolean bleEnabled;
    private boolean wifiDirectEnabled;
    private boolean internetEnabled;
    private boolean reticulumEnabled;
    private boolean nostrEnabled;
    private boolean nostrSealingEnabled;
    private boolean nostrColdContactEnabled;
    private boolean nostrUsernameDiscoveryEnabled;

    public TransportConfig(
        boolean bleEnabled, 
        boolean wifiDirectEnabled, 
        boolean internetEnabled, 
        boolean reticulumEnabled, 
        boolean nostrEnabled, 
        boolean nostrSealingEnabled, 
        boolean nostrColdContactEnabled, 
        boolean nostrUsernameDiscoveryEnabled
    ) {
        
        this.bleEnabled = bleEnabled;
        
        this.wifiDirectEnabled = wifiDirectEnabled;
        
        this.internetEnabled = internetEnabled;
        
        this.reticulumEnabled = reticulumEnabled;
        
        this.nostrEnabled = nostrEnabled;
        
        this.nostrSealingEnabled = nostrSealingEnabled;
        
        this.nostrColdContactEnabled = nostrColdContactEnabled;
        
        this.nostrUsernameDiscoveryEnabled = nostrUsernameDiscoveryEnabled;
    }
    
    public boolean bleEnabled() {
        return this.bleEnabled;
    }
    
    public boolean wifiDirectEnabled() {
        return this.wifiDirectEnabled;
    }
    
    public boolean internetEnabled() {
        return this.internetEnabled;
    }
    
    public boolean reticulumEnabled() {
        return this.reticulumEnabled;
    }
    
    public boolean nostrEnabled() {
        return this.nostrEnabled;
    }
    
    public boolean nostrSealingEnabled() {
        return this.nostrSealingEnabled;
    }
    
    public boolean nostrColdContactEnabled() {
        return this.nostrColdContactEnabled;
    }
    
    public boolean nostrUsernameDiscoveryEnabled() {
        return this.nostrUsernameDiscoveryEnabled;
    }
    public void setBleEnabled(boolean bleEnabled) {
        this.bleEnabled = bleEnabled;
    }
    public void setWifiDirectEnabled(boolean wifiDirectEnabled) {
        this.wifiDirectEnabled = wifiDirectEnabled;
    }
    public void setInternetEnabled(boolean internetEnabled) {
        this.internetEnabled = internetEnabled;
    }
    public void setReticulumEnabled(boolean reticulumEnabled) {
        this.reticulumEnabled = reticulumEnabled;
    }
    public void setNostrEnabled(boolean nostrEnabled) {
        this.nostrEnabled = nostrEnabled;
    }
    public void setNostrSealingEnabled(boolean nostrSealingEnabled) {
        this.nostrSealingEnabled = nostrSealingEnabled;
    }
    public void setNostrColdContactEnabled(boolean nostrColdContactEnabled) {
        this.nostrColdContactEnabled = nostrColdContactEnabled;
    }
    public void setNostrUsernameDiscoveryEnabled(boolean nostrUsernameDiscoveryEnabled) {
        this.nostrUsernameDiscoveryEnabled = nostrUsernameDiscoveryEnabled;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof TransportConfig) {
            TransportConfig _uniffiThat = (TransportConfig) _uniffiOther;
            return (
              bleEnabled == _uniffiThat.bleEnabled && 
              
              wifiDirectEnabled == _uniffiThat.wifiDirectEnabled && 
              
              internetEnabled == _uniffiThat.internetEnabled && 
              
              reticulumEnabled == _uniffiThat.reticulumEnabled && 
              
              nostrEnabled == _uniffiThat.nostrEnabled && 
              
              nostrSealingEnabled == _uniffiThat.nostrSealingEnabled && 
              
              nostrColdContactEnabled == _uniffiThat.nostrColdContactEnabled && 
              
              nostrUsernameDiscoveryEnabled == _uniffiThat.nostrUsernameDiscoveryEnabled
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(bleEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(wifiDirectEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(internetEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(reticulumEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrSealingEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrColdContactEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrUsernameDiscoveryEnabled);
        return _uniffiHash;
    }

    
    
    

}


