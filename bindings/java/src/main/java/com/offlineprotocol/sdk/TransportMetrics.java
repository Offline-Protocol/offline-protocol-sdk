package com.offlineprotocol.sdk;

public class TransportMetrics {
    private int packetsSent;
    private int packetsReceived;
    private int bytesSent;
    private int bytesReceived;
    private float errorRate;
    private int avgLatencyMs;
    private java.lang.Short rssi;
    private java.lang.Long bandwidthBps;
    private java.lang.Float congestion;
    private java.lang.Integer queueDepth;
    private java.lang.Byte batteryLevel;
    private java.lang.Boolean isCharging;
    private java.lang.Byte relayConnectionCount;
    private java.lang.Boolean isActiveRelay;
    private java.lang.Float deliveryRatio;
    private java.lang.Float dropRate;
    private java.lang.Float averageHopCount;
    private java.lang.Float energyCost;

    public TransportMetrics(
        int packetsSent, 
        int packetsReceived, 
        int bytesSent, 
        int bytesReceived, 
        float errorRate, 
        int avgLatencyMs, 
        java.lang.Short rssi, 
        java.lang.Long bandwidthBps, 
        java.lang.Float congestion, 
        java.lang.Integer queueDepth, 
        java.lang.Byte batteryLevel, 
        java.lang.Boolean isCharging, 
        java.lang.Byte relayConnectionCount, 
        java.lang.Boolean isActiveRelay, 
        java.lang.Float deliveryRatio, 
        java.lang.Float dropRate, 
        java.lang.Float averageHopCount, 
        java.lang.Float energyCost
    ) {
        
        this.packetsSent = packetsSent;
        
        this.packetsReceived = packetsReceived;
        
        this.bytesSent = bytesSent;
        
        this.bytesReceived = bytesReceived;
        
        this.errorRate = errorRate;
        
        this.avgLatencyMs = avgLatencyMs;
        
        this.rssi = rssi;
        
        this.bandwidthBps = bandwidthBps;
        
        this.congestion = congestion;
        
        this.queueDepth = queueDepth;
        
        this.batteryLevel = batteryLevel;
        
        this.isCharging = isCharging;
        
        this.relayConnectionCount = relayConnectionCount;
        
        this.isActiveRelay = isActiveRelay;
        
        this.deliveryRatio = deliveryRatio;
        
        this.dropRate = dropRate;
        
        this.averageHopCount = averageHopCount;
        
        this.energyCost = energyCost;
    }
    
    public int packetsSent() {
        return this.packetsSent;
    }
    
    public int packetsReceived() {
        return this.packetsReceived;
    }
    
    public int bytesSent() {
        return this.bytesSent;
    }
    
    public int bytesReceived() {
        return this.bytesReceived;
    }
    
    public float errorRate() {
        return this.errorRate;
    }
    
    public int avgLatencyMs() {
        return this.avgLatencyMs;
    }
    
    public java.lang.Short rssi() {
        return this.rssi;
    }
    
    public java.lang.Long bandwidthBps() {
        return this.bandwidthBps;
    }
    
    public java.lang.Float congestion() {
        return this.congestion;
    }
    
    public java.lang.Integer queueDepth() {
        return this.queueDepth;
    }
    
    public java.lang.Byte batteryLevel() {
        return this.batteryLevel;
    }
    
    public java.lang.Boolean isCharging() {
        return this.isCharging;
    }
    
    public java.lang.Byte relayConnectionCount() {
        return this.relayConnectionCount;
    }
    
    public java.lang.Boolean isActiveRelay() {
        return this.isActiveRelay;
    }
    
    public java.lang.Float deliveryRatio() {
        return this.deliveryRatio;
    }
    
    public java.lang.Float dropRate() {
        return this.dropRate;
    }
    
    public java.lang.Float averageHopCount() {
        return this.averageHopCount;
    }
    
    public java.lang.Float energyCost() {
        return this.energyCost;
    }
    public void setPacketsSent(int packetsSent) {
        this.packetsSent = packetsSent;
    }
    public void setPacketsReceived(int packetsReceived) {
        this.packetsReceived = packetsReceived;
    }
    public void setBytesSent(int bytesSent) {
        this.bytesSent = bytesSent;
    }
    public void setBytesReceived(int bytesReceived) {
        this.bytesReceived = bytesReceived;
    }
    public void setErrorRate(float errorRate) {
        this.errorRate = errorRate;
    }
    public void setAvgLatencyMs(int avgLatencyMs) {
        this.avgLatencyMs = avgLatencyMs;
    }
    public void setRssi(java.lang.Short rssi) {
        this.rssi = rssi;
    }
    public void setBandwidthBps(java.lang.Long bandwidthBps) {
        this.bandwidthBps = bandwidthBps;
    }
    public void setCongestion(java.lang.Float congestion) {
        this.congestion = congestion;
    }
    public void setQueueDepth(java.lang.Integer queueDepth) {
        this.queueDepth = queueDepth;
    }
    public void setBatteryLevel(java.lang.Byte batteryLevel) {
        this.batteryLevel = batteryLevel;
    }
    public void setIsCharging(java.lang.Boolean isCharging) {
        this.isCharging = isCharging;
    }
    public void setRelayConnectionCount(java.lang.Byte relayConnectionCount) {
        this.relayConnectionCount = relayConnectionCount;
    }
    public void setIsActiveRelay(java.lang.Boolean isActiveRelay) {
        this.isActiveRelay = isActiveRelay;
    }
    public void setDeliveryRatio(java.lang.Float deliveryRatio) {
        this.deliveryRatio = deliveryRatio;
    }
    public void setDropRate(java.lang.Float dropRate) {
        this.dropRate = dropRate;
    }
    public void setAverageHopCount(java.lang.Float averageHopCount) {
        this.averageHopCount = averageHopCount;
    }
    public void setEnergyCost(java.lang.Float energyCost) {
        this.energyCost = energyCost;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof TransportMetrics) {
            TransportMetrics _uniffiThat = (TransportMetrics) _uniffiOther;
            return (
              packetsSent == _uniffiThat.packetsSent && 
              
              packetsReceived == _uniffiThat.packetsReceived && 
              
              bytesSent == _uniffiThat.bytesSent && 
              
              bytesReceived == _uniffiThat.bytesReceived && 
              
              java.lang.Float.compare(errorRate, _uniffiThat.errorRate) == 0 && 
              
              avgLatencyMs == _uniffiThat.avgLatencyMs && 
              
              java.util.Objects.equals(rssi, _uniffiThat.rssi) && 
              
              java.util.Objects.equals(bandwidthBps, _uniffiThat.bandwidthBps) && 
              
              java.util.Objects.equals(congestion, _uniffiThat.congestion) && 
              
              java.util.Objects.equals(queueDepth, _uniffiThat.queueDepth) && 
              
              java.util.Objects.equals(batteryLevel, _uniffiThat.batteryLevel) && 
              
              java.util.Objects.equals(isCharging, _uniffiThat.isCharging) && 
              
              java.util.Objects.equals(relayConnectionCount, _uniffiThat.relayConnectionCount) && 
              
              java.util.Objects.equals(isActiveRelay, _uniffiThat.isActiveRelay) && 
              
              java.util.Objects.equals(deliveryRatio, _uniffiThat.deliveryRatio) && 
              
              java.util.Objects.equals(dropRate, _uniffiThat.dropRate) && 
              
              java.util.Objects.equals(averageHopCount, _uniffiThat.averageHopCount) && 
              
              java.util.Objects.equals(energyCost, _uniffiThat.energyCost)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(packetsSent);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(packetsReceived);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(bytesSent);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(bytesReceived);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(errorRate);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(avgLatencyMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(rssi);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(bandwidthBps);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(congestion);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(queueDepth);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(batteryLevel);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(isCharging);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(relayConnectionCount);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(isActiveRelay);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(deliveryRatio);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(dropRate);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(averageHopCount);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(energyCost);
        return _uniffiHash;
    }

    
    
    

}


