package com.offlineprotocol.sdk;

public class WebrtcDirectEvent {
    private WebrtcDirectEventKind kind;
    private long stream;
    private java.lang.String address;
    private java.lang.String remoteIp;
    private java.lang.Short remotePort;
    private java.util.List<java.lang.Byte> data;

    public WebrtcDirectEvent(
        WebrtcDirectEventKind kind, 
        long stream, 
        java.lang.String address, 
        java.lang.String remoteIp, 
        java.lang.Short remotePort, 
        java.util.List<java.lang.Byte> data
    ) {
        
        this.kind = kind;
        
        this.stream = stream;
        
        this.address = address;
        
        this.remoteIp = remoteIp;
        
        this.remotePort = remotePort;
        
        this.data = data;
    }
    
    public WebrtcDirectEventKind kind() {
        return this.kind;
    }
    
    public long stream() {
        return this.stream;
    }
    
    public java.lang.String address() {
        return this.address;
    }
    
    public java.lang.String remoteIp() {
        return this.remoteIp;
    }
    
    public java.lang.Short remotePort() {
        return this.remotePort;
    }
    
    public java.util.List<java.lang.Byte> data() {
        return this.data;
    }
    public void setKind(WebrtcDirectEventKind kind) {
        this.kind = kind;
    }
    public void setStream(long stream) {
        this.stream = stream;
    }
    public void setAddress(java.lang.String address) {
        this.address = address;
    }
    public void setRemoteIp(java.lang.String remoteIp) {
        this.remoteIp = remoteIp;
    }
    public void setRemotePort(java.lang.Short remotePort) {
        this.remotePort = remotePort;
    }
    public void setData(java.util.List<java.lang.Byte> data) {
        this.data = data;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof WebrtcDirectEvent) {
            WebrtcDirectEvent _uniffiThat = (WebrtcDirectEvent) _uniffiOther;
            return (
              java.util.Objects.equals(kind, _uniffiThat.kind) && 
              
              stream == _uniffiThat.stream && 
              
              java.util.Objects.equals(address, _uniffiThat.address) && 
              
              java.util.Objects.equals(remoteIp, _uniffiThat.remoteIp) && 
              
              java.util.Objects.equals(remotePort, _uniffiThat.remotePort) && 
              
              java.util.Objects.equals(data, _uniffiThat.data)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(kind);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(stream);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(address);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(remoteIp);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(remotePort);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(data);
        return _uniffiHash;
    }

    
    
    

}


