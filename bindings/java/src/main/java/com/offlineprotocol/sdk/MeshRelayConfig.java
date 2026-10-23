package com.offlineprotocol.sdk;

public class MeshRelayConfig {
    private java.lang.Byte maxTtl;
    private java.lang.Byte denseMaxTtl;
    private java.lang.Long denseDegree;
    private java.lang.Long fanout;
    private java.lang.Long jitterMinMs;
    private java.lang.Long jitterMaxMs;
    private java.lang.Float ratePerSec;
    private java.lang.Float burst;
    private java.lang.Float peerRatePerSec;
    private java.lang.Float peerBurst;
    private java.lang.Long queueCapacity;
    private java.lang.Float biasMinScale;
    private java.lang.Long biasMaxHandicapMs;
    private java.lang.Long activityWindowMs;
    private java.lang.Long activityMinForwards;
    private java.lang.Integer activityIdleWindows;

    public MeshRelayConfig(
        java.lang.Byte maxTtl, 
        java.lang.Byte denseMaxTtl, 
        java.lang.Long denseDegree, 
        java.lang.Long fanout, 
        java.lang.Long jitterMinMs, 
        java.lang.Long jitterMaxMs, 
        java.lang.Float ratePerSec, 
        java.lang.Float burst, 
        java.lang.Float peerRatePerSec, 
        java.lang.Float peerBurst, 
        java.lang.Long queueCapacity, 
        java.lang.Float biasMinScale, 
        java.lang.Long biasMaxHandicapMs, 
        java.lang.Long activityWindowMs, 
        java.lang.Long activityMinForwards, 
        java.lang.Integer activityIdleWindows
    ) {
        
        this.maxTtl = maxTtl;
        
        this.denseMaxTtl = denseMaxTtl;
        
        this.denseDegree = denseDegree;
        
        this.fanout = fanout;
        
        this.jitterMinMs = jitterMinMs;
        
        this.jitterMaxMs = jitterMaxMs;
        
        this.ratePerSec = ratePerSec;
        
        this.burst = burst;
        
        this.peerRatePerSec = peerRatePerSec;
        
        this.peerBurst = peerBurst;
        
        this.queueCapacity = queueCapacity;
        
        this.biasMinScale = biasMinScale;
        
        this.biasMaxHandicapMs = biasMaxHandicapMs;
        
        this.activityWindowMs = activityWindowMs;
        
        this.activityMinForwards = activityMinForwards;
        
        this.activityIdleWindows = activityIdleWindows;
    }
    
    public java.lang.Byte maxTtl() {
        return this.maxTtl;
    }
    
    public java.lang.Byte denseMaxTtl() {
        return this.denseMaxTtl;
    }
    
    public java.lang.Long denseDegree() {
        return this.denseDegree;
    }
    
    public java.lang.Long fanout() {
        return this.fanout;
    }
    
    public java.lang.Long jitterMinMs() {
        return this.jitterMinMs;
    }
    
    public java.lang.Long jitterMaxMs() {
        return this.jitterMaxMs;
    }
    
    public java.lang.Float ratePerSec() {
        return this.ratePerSec;
    }
    
    public java.lang.Float burst() {
        return this.burst;
    }
    
    public java.lang.Float peerRatePerSec() {
        return this.peerRatePerSec;
    }
    
    public java.lang.Float peerBurst() {
        return this.peerBurst;
    }
    
    public java.lang.Long queueCapacity() {
        return this.queueCapacity;
    }
    
    public java.lang.Float biasMinScale() {
        return this.biasMinScale;
    }
    
    public java.lang.Long biasMaxHandicapMs() {
        return this.biasMaxHandicapMs;
    }
    
    public java.lang.Long activityWindowMs() {
        return this.activityWindowMs;
    }
    
    public java.lang.Long activityMinForwards() {
        return this.activityMinForwards;
    }
    
    public java.lang.Integer activityIdleWindows() {
        return this.activityIdleWindows;
    }
    public void setMaxTtl(java.lang.Byte maxTtl) {
        this.maxTtl = maxTtl;
    }
    public void setDenseMaxTtl(java.lang.Byte denseMaxTtl) {
        this.denseMaxTtl = denseMaxTtl;
    }
    public void setDenseDegree(java.lang.Long denseDegree) {
        this.denseDegree = denseDegree;
    }
    public void setFanout(java.lang.Long fanout) {
        this.fanout = fanout;
    }
    public void setJitterMinMs(java.lang.Long jitterMinMs) {
        this.jitterMinMs = jitterMinMs;
    }
    public void setJitterMaxMs(java.lang.Long jitterMaxMs) {
        this.jitterMaxMs = jitterMaxMs;
    }
    public void setRatePerSec(java.lang.Float ratePerSec) {
        this.ratePerSec = ratePerSec;
    }
    public void setBurst(java.lang.Float burst) {
        this.burst = burst;
    }
    public void setPeerRatePerSec(java.lang.Float peerRatePerSec) {
        this.peerRatePerSec = peerRatePerSec;
    }
    public void setPeerBurst(java.lang.Float peerBurst) {
        this.peerBurst = peerBurst;
    }
    public void setQueueCapacity(java.lang.Long queueCapacity) {
        this.queueCapacity = queueCapacity;
    }
    public void setBiasMinScale(java.lang.Float biasMinScale) {
        this.biasMinScale = biasMinScale;
    }
    public void setBiasMaxHandicapMs(java.lang.Long biasMaxHandicapMs) {
        this.biasMaxHandicapMs = biasMaxHandicapMs;
    }
    public void setActivityWindowMs(java.lang.Long activityWindowMs) {
        this.activityWindowMs = activityWindowMs;
    }
    public void setActivityMinForwards(java.lang.Long activityMinForwards) {
        this.activityMinForwards = activityMinForwards;
    }
    public void setActivityIdleWindows(java.lang.Integer activityIdleWindows) {
        this.activityIdleWindows = activityIdleWindows;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MeshRelayConfig) {
            MeshRelayConfig _uniffiThat = (MeshRelayConfig) _uniffiOther;
            return (
              java.util.Objects.equals(maxTtl, _uniffiThat.maxTtl) && 
              
              java.util.Objects.equals(denseMaxTtl, _uniffiThat.denseMaxTtl) && 
              
              java.util.Objects.equals(denseDegree, _uniffiThat.denseDegree) && 
              
              java.util.Objects.equals(fanout, _uniffiThat.fanout) && 
              
              java.util.Objects.equals(jitterMinMs, _uniffiThat.jitterMinMs) && 
              
              java.util.Objects.equals(jitterMaxMs, _uniffiThat.jitterMaxMs) && 
              
              java.util.Objects.equals(ratePerSec, _uniffiThat.ratePerSec) && 
              
              java.util.Objects.equals(burst, _uniffiThat.burst) && 
              
              java.util.Objects.equals(peerRatePerSec, _uniffiThat.peerRatePerSec) && 
              
              java.util.Objects.equals(peerBurst, _uniffiThat.peerBurst) && 
              
              java.util.Objects.equals(queueCapacity, _uniffiThat.queueCapacity) && 
              
              java.util.Objects.equals(biasMinScale, _uniffiThat.biasMinScale) && 
              
              java.util.Objects.equals(biasMaxHandicapMs, _uniffiThat.biasMaxHandicapMs) && 
              
              java.util.Objects.equals(activityWindowMs, _uniffiThat.activityWindowMs) && 
              
              java.util.Objects.equals(activityMinForwards, _uniffiThat.activityMinForwards) && 
              
              java.util.Objects.equals(activityIdleWindows, _uniffiThat.activityIdleWindows)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxTtl);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(denseMaxTtl);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(denseDegree);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(fanout);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(jitterMinMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(jitterMaxMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(ratePerSec);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(burst);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(peerRatePerSec);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(peerBurst);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(queueCapacity);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(biasMinScale);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(biasMaxHandicapMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(activityWindowMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(activityMinForwards);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(activityIdleWindows);
        return _uniffiHash;
    }

    
    
    

}


