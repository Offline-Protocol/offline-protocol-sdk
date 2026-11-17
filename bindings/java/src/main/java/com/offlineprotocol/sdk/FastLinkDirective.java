package com.offlineprotocol.sdk;

public class FastLinkDirective {
    private java.lang.String kind;
    private java.lang.String linkId;
    private java.lang.String peerId;
    private java.lang.String ssid;
    private java.lang.String passphrase;
    private java.lang.String host;
    private java.lang.Short port;
    private long expiresInMs;
    private java.lang.String reason;

    public FastLinkDirective(
        java.lang.String kind, 
        java.lang.String linkId, 
        java.lang.String peerId, 
        java.lang.String ssid, 
        java.lang.String passphrase, 
        java.lang.String host, 
        java.lang.Short port, 
        long expiresInMs, 
        java.lang.String reason
    ) {
        
        this.kind = kind;
        
        this.linkId = linkId;
        
        this.peerId = peerId;
        
        this.ssid = ssid;
        
        this.passphrase = passphrase;
        
        this.host = host;
        
        this.port = port;
        
        this.expiresInMs = expiresInMs;
        
        this.reason = reason;
    }
    
    public java.lang.String kind() {
        return this.kind;
    }
    
    public java.lang.String linkId() {
        return this.linkId;
    }
    
    public java.lang.String peerId() {
        return this.peerId;
    }
    
    public java.lang.String ssid() {
        return this.ssid;
    }
    
    public java.lang.String passphrase() {
        return this.passphrase;
    }
    
    public java.lang.String host() {
        return this.host;
    }
    
    public java.lang.Short port() {
        return this.port;
    }
    
    public long expiresInMs() {
        return this.expiresInMs;
    }
    
    public java.lang.String reason() {
        return this.reason;
    }
    public void setKind(java.lang.String kind) {
        this.kind = kind;
    }
    public void setLinkId(java.lang.String linkId) {
        this.linkId = linkId;
    }
    public void setPeerId(java.lang.String peerId) {
        this.peerId = peerId;
    }
    public void setSsid(java.lang.String ssid) {
        this.ssid = ssid;
    }
    public void setPassphrase(java.lang.String passphrase) {
        this.passphrase = passphrase;
    }
    public void setHost(java.lang.String host) {
        this.host = host;
    }
    public void setPort(java.lang.Short port) {
        this.port = port;
    }
    public void setExpiresInMs(long expiresInMs) {
        this.expiresInMs = expiresInMs;
    }
    public void setReason(java.lang.String reason) {
        this.reason = reason;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof FastLinkDirective) {
            FastLinkDirective _uniffiThat = (FastLinkDirective) _uniffiOther;
            return (
              java.util.Objects.equals(kind, _uniffiThat.kind) && 
              
              java.util.Objects.equals(linkId, _uniffiThat.linkId) && 
              
              java.util.Objects.equals(peerId, _uniffiThat.peerId) && 
              
              java.util.Objects.equals(ssid, _uniffiThat.ssid) && 
              
              java.util.Objects.equals(passphrase, _uniffiThat.passphrase) && 
              
              java.util.Objects.equals(host, _uniffiThat.host) && 
              
              java.util.Objects.equals(port, _uniffiThat.port) && 
              
              expiresInMs == _uniffiThat.expiresInMs && 
              
              java.util.Objects.equals(reason, _uniffiThat.reason)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(kind);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(linkId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(peerId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(ssid);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(passphrase);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(host);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(port);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(expiresInMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(reason);
        return _uniffiHash;
    }

    
    
    

}


