package com.offlineprotocol.sdk;

public class WebrtcDirectDatagram {
    private java.lang.String destinationIp;
    private short destinationPort;
    private java.util.List<java.lang.Byte> data;

    public WebrtcDirectDatagram(
        java.lang.String destinationIp, 
        short destinationPort, 
        java.util.List<java.lang.Byte> data
    ) {
        
        this.destinationIp = destinationIp;
        
        this.destinationPort = destinationPort;
        
        this.data = data;
    }
    
    public java.lang.String destinationIp() {
        return this.destinationIp;
    }
    
    public short destinationPort() {
        return this.destinationPort;
    }
    
    public java.util.List<java.lang.Byte> data() {
        return this.data;
    }
    public void setDestinationIp(java.lang.String destinationIp) {
        this.destinationIp = destinationIp;
    }
    public void setDestinationPort(short destinationPort) {
        this.destinationPort = destinationPort;
    }
    public void setData(java.util.List<java.lang.Byte> data) {
        this.data = data;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof WebrtcDirectDatagram) {
            WebrtcDirectDatagram _uniffiThat = (WebrtcDirectDatagram) _uniffiOther;
            return (
              java.util.Objects.equals(destinationIp, _uniffiThat.destinationIp) && 
              
              destinationPort == _uniffiThat.destinationPort && 
              
              java.util.Objects.equals(data, _uniffiThat.data)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(destinationIp);
        _uniffiHash = 31 * _uniffiHash + java.lang.Short.hashCode(destinationPort);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(data);
        return _uniffiHash;
    }

    
    
    

}


