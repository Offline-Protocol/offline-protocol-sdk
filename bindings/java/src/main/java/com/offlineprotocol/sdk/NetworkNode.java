package com.offlineprotocol.sdk;

public class NetworkNode {
    private java.lang.String nodeId;
    private java.lang.String role;
    private java.lang.Short rssi;
    private java.lang.Byte batteryLevel;
    private int connectionCount;
    private long lastSeenMs;

    public NetworkNode(
        java.lang.String nodeId, 
        java.lang.String role, 
        java.lang.Short rssi, 
        java.lang.Byte batteryLevel, 
        int connectionCount, 
        long lastSeenMs
    ) {
        
        this.nodeId = nodeId;
        
        this.role = role;
        
        this.rssi = rssi;
        
        this.batteryLevel = batteryLevel;
        
        this.connectionCount = connectionCount;
        
        this.lastSeenMs = lastSeenMs;
    }
    
    public java.lang.String nodeId() {
        return this.nodeId;
    }
    
    public java.lang.String role() {
        return this.role;
    }
    
    public java.lang.Short rssi() {
        return this.rssi;
    }
    
    public java.lang.Byte batteryLevel() {
        return this.batteryLevel;
    }
    
    public int connectionCount() {
        return this.connectionCount;
    }
    
    public long lastSeenMs() {
        return this.lastSeenMs;
    }
    public void setNodeId(java.lang.String nodeId) {
        this.nodeId = nodeId;
    }
    public void setRole(java.lang.String role) {
        this.role = role;
    }
    public void setRssi(java.lang.Short rssi) {
        this.rssi = rssi;
    }
    public void setBatteryLevel(java.lang.Byte batteryLevel) {
        this.batteryLevel = batteryLevel;
    }
    public void setConnectionCount(int connectionCount) {
        this.connectionCount = connectionCount;
    }
    public void setLastSeenMs(long lastSeenMs) {
        this.lastSeenMs = lastSeenMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof NetworkNode) {
            NetworkNode _uniffiThat = (NetworkNode) _uniffiOther;
            return (
              java.util.Objects.equals(nodeId, _uniffiThat.nodeId) && 
              
              java.util.Objects.equals(role, _uniffiThat.role) && 
              
              java.util.Objects.equals(rssi, _uniffiThat.rssi) && 
              
              java.util.Objects.equals(batteryLevel, _uniffiThat.batteryLevel) && 
              
              connectionCount == _uniffiThat.connectionCount && 
              
              lastSeenMs == _uniffiThat.lastSeenMs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(nodeId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(role);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(rssi);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(batteryLevel);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(connectionCount);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(lastSeenMs);
        return _uniffiHash;
    }

    
    
    

}


