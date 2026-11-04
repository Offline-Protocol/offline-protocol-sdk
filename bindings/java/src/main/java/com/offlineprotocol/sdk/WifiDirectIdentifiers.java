package com.offlineprotocol.sdk;

public class WifiDirectIdentifiers {
    private java.lang.String recordApp;
    private java.lang.String networkName;
    private java.lang.String passphrase;

    public WifiDirectIdentifiers(
        java.lang.String recordApp, 
        java.lang.String networkName, 
        java.lang.String passphrase
    ) {
        
        this.recordApp = recordApp;
        
        this.networkName = networkName;
        
        this.passphrase = passphrase;
    }
    
    public java.lang.String recordApp() {
        return this.recordApp;
    }
    
    public java.lang.String networkName() {
        return this.networkName;
    }
    
    public java.lang.String passphrase() {
        return this.passphrase;
    }
    public void setRecordApp(java.lang.String recordApp) {
        this.recordApp = recordApp;
    }
    public void setNetworkName(java.lang.String networkName) {
        this.networkName = networkName;
    }
    public void setPassphrase(java.lang.String passphrase) {
        this.passphrase = passphrase;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof WifiDirectIdentifiers) {
            WifiDirectIdentifiers _uniffiThat = (WifiDirectIdentifiers) _uniffiOther;
            return (
              java.util.Objects.equals(recordApp, _uniffiThat.recordApp) && 
              
              java.util.Objects.equals(networkName, _uniffiThat.networkName) && 
              
              java.util.Objects.equals(passphrase, _uniffiThat.passphrase)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(recordApp);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(networkName);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(passphrase);
        return _uniffiHash;
    }

    
    
    

}


