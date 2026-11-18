package com.offlineprotocol.sdk;

public class WebrtcDirectCode {
    private java.lang.String fragment;
    private java.lang.String host;
    private short port;
    private java.lang.String address;

    public WebrtcDirectCode(
        java.lang.String fragment, 
        java.lang.String host, 
        short port, 
        java.lang.String address
    ) {
        
        this.fragment = fragment;
        
        this.host = host;
        
        this.port = port;
        
        this.address = address;
    }
    
    public java.lang.String fragment() {
        return this.fragment;
    }
    
    public java.lang.String host() {
        return this.host;
    }
    
    public short port() {
        return this.port;
    }
    
    public java.lang.String address() {
        return this.address;
    }
    public void setFragment(java.lang.String fragment) {
        this.fragment = fragment;
    }
    public void setHost(java.lang.String host) {
        this.host = host;
    }
    public void setPort(short port) {
        this.port = port;
    }
    public void setAddress(java.lang.String address) {
        this.address = address;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof WebrtcDirectCode) {
            WebrtcDirectCode _uniffiThat = (WebrtcDirectCode) _uniffiOther;
            return (
              java.util.Objects.equals(fragment, _uniffiThat.fragment) && 
              
              java.util.Objects.equals(host, _uniffiThat.host) && 
              
              port == _uniffiThat.port && 
              
              java.util.Objects.equals(address, _uniffiThat.address)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(fragment);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(host);
        _uniffiHash = 31 * _uniffiHash + java.lang.Short.hashCode(port);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(address);
        return _uniffiHash;
    }

    
    
    

}


