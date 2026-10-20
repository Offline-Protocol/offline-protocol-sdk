package com.offlineprotocol.sdk;

public class NetworkLink {
    private java.lang.String sourceId;
    private java.lang.String targetId;
    private java.lang.String transport;
    private float quality;

    public NetworkLink(
        java.lang.String sourceId, 
        java.lang.String targetId, 
        java.lang.String transport, 
        float quality
    ) {
        
        this.sourceId = sourceId;
        
        this.targetId = targetId;
        
        this.transport = transport;
        
        this.quality = quality;
    }
    
    public java.lang.String sourceId() {
        return this.sourceId;
    }
    
    public java.lang.String targetId() {
        return this.targetId;
    }
    
    public java.lang.String transport() {
        return this.transport;
    }
    
    public float quality() {
        return this.quality;
    }
    public void setSourceId(java.lang.String sourceId) {
        this.sourceId = sourceId;
    }
    public void setTargetId(java.lang.String targetId) {
        this.targetId = targetId;
    }
    public void setTransport(java.lang.String transport) {
        this.transport = transport;
    }
    public void setQuality(float quality) {
        this.quality = quality;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof NetworkLink) {
            NetworkLink _uniffiThat = (NetworkLink) _uniffiOther;
            return (
              java.util.Objects.equals(sourceId, _uniffiThat.sourceId) && 
              
              java.util.Objects.equals(targetId, _uniffiThat.targetId) && 
              
              java.util.Objects.equals(transport, _uniffiThat.transport) && 
              
              java.lang.Float.compare(quality, _uniffiThat.quality) == 0
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(sourceId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(targetId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(transport);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(quality);
        return _uniffiHash;
    }

    
    
    

}


