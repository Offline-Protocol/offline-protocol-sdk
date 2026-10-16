package com.offlineprotocol.sdk;

public class PeerDevice {
    private java.lang.String peerId;
    private short rssi;
    private long lastSeenMs;

    public PeerDevice(
        java.lang.String peerId, 
        short rssi, 
        long lastSeenMs
    ) {
        
        this.peerId = peerId;
        
        this.rssi = rssi;
        
        this.lastSeenMs = lastSeenMs;
    }
    
    public java.lang.String peerId() {
        return this.peerId;
    }
    
    public short rssi() {
        return this.rssi;
    }
    
    public long lastSeenMs() {
        return this.lastSeenMs;
    }
    public void setPeerId(java.lang.String peerId) {
        this.peerId = peerId;
    }
    public void setRssi(short rssi) {
        this.rssi = rssi;
    }
    public void setLastSeenMs(long lastSeenMs) {
        this.lastSeenMs = lastSeenMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof PeerDevice) {
            PeerDevice _uniffiThat = (PeerDevice) _uniffiOther;
            return (
              java.util.Objects.equals(peerId, _uniffiThat.peerId) && 
              
              rssi == _uniffiThat.rssi && 
              
              lastSeenMs == _uniffiThat.lastSeenMs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(peerId);
        _uniffiHash = 31 * _uniffiHash + java.lang.Short.hashCode(rssi);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(lastSeenMs);
        return _uniffiHash;
    }

    
    
    

}


