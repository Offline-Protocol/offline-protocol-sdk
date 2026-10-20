package com.offlineprotocol.sdk;

public class MeshRelayTunables {
    private byte maxTtl;
    private byte denseMaxTtl;
    private long denseDegree;
    private long fanout;
    private long jitterMinMs;
    private long jitterMaxMs;
    private float ratePerSec;
    private float burst;
    private float peerRatePerSec;
    private float peerBurst;
    private long queueCapacity;
    private float biasMinScale;
    private long biasMaxHandicapMs;
    private long activityWindowMs;
    private long activityMinForwards;
    private int activityIdleWindows;

    public MeshRelayTunables(
        byte maxTtl, 
        byte denseMaxTtl, 
        long denseDegree, 
        long fanout, 
        long jitterMinMs, 
        long jitterMaxMs, 
        float ratePerSec, 
        float burst, 
        float peerRatePerSec, 
        float peerBurst, 
        long queueCapacity, 
        float biasMinScale, 
        long biasMaxHandicapMs, 
        long activityWindowMs, 
        long activityMinForwards, 
        int activityIdleWindows
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
    
    public byte maxTtl() {
        return this.maxTtl;
    }
    
    public byte denseMaxTtl() {
        return this.denseMaxTtl;
    }
    
    public long denseDegree() {
        return this.denseDegree;
    }
    
    public long fanout() {
        return this.fanout;
    }
    
    public long jitterMinMs() {
        return this.jitterMinMs;
    }
    
    public long jitterMaxMs() {
        return this.jitterMaxMs;
    }
    
    public float ratePerSec() {
        return this.ratePerSec;
    }
    
    public float burst() {
        return this.burst;
    }
    
    public float peerRatePerSec() {
        return this.peerRatePerSec;
    }
    
    public float peerBurst() {
        return this.peerBurst;
    }
    
    public long queueCapacity() {
        return this.queueCapacity;
    }
    
    public float biasMinScale() {
        return this.biasMinScale;
    }
    
    public long biasMaxHandicapMs() {
        return this.biasMaxHandicapMs;
    }
    
    public long activityWindowMs() {
        return this.activityWindowMs;
    }
    
    public long activityMinForwards() {
        return this.activityMinForwards;
    }
    
    public int activityIdleWindows() {
        return this.activityIdleWindows;
    }
    public void setMaxTtl(byte maxTtl) {
        this.maxTtl = maxTtl;
    }
    public void setDenseMaxTtl(byte denseMaxTtl) {
        this.denseMaxTtl = denseMaxTtl;
    }
    public void setDenseDegree(long denseDegree) {
        this.denseDegree = denseDegree;
    }
    public void setFanout(long fanout) {
        this.fanout = fanout;
    }
    public void setJitterMinMs(long jitterMinMs) {
        this.jitterMinMs = jitterMinMs;
    }
    public void setJitterMaxMs(long jitterMaxMs) {
        this.jitterMaxMs = jitterMaxMs;
    }
    public void setRatePerSec(float ratePerSec) {
        this.ratePerSec = ratePerSec;
    }
    public void setBurst(float burst) {
        this.burst = burst;
    }
    public void setPeerRatePerSec(float peerRatePerSec) {
        this.peerRatePerSec = peerRatePerSec;
    }
    public void setPeerBurst(float peerBurst) {
        this.peerBurst = peerBurst;
    }
    public void setQueueCapacity(long queueCapacity) {
        this.queueCapacity = queueCapacity;
    }
    public void setBiasMinScale(float biasMinScale) {
        this.biasMinScale = biasMinScale;
    }
    public void setBiasMaxHandicapMs(long biasMaxHandicapMs) {
        this.biasMaxHandicapMs = biasMaxHandicapMs;
    }
    public void setActivityWindowMs(long activityWindowMs) {
        this.activityWindowMs = activityWindowMs;
    }
    public void setActivityMinForwards(long activityMinForwards) {
        this.activityMinForwards = activityMinForwards;
    }
    public void setActivityIdleWindows(int activityIdleWindows) {
        this.activityIdleWindows = activityIdleWindows;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MeshRelayTunables) {
            MeshRelayTunables _uniffiThat = (MeshRelayTunables) _uniffiOther;
            return (
              maxTtl == _uniffiThat.maxTtl && 
              
              denseMaxTtl == _uniffiThat.denseMaxTtl && 
              
              denseDegree == _uniffiThat.denseDegree && 
              
              fanout == _uniffiThat.fanout && 
              
              jitterMinMs == _uniffiThat.jitterMinMs && 
              
              jitterMaxMs == _uniffiThat.jitterMaxMs && 
              
              java.lang.Float.compare(ratePerSec, _uniffiThat.ratePerSec) == 0 && 
              
              java.lang.Float.compare(burst, _uniffiThat.burst) == 0 && 
              
              java.lang.Float.compare(peerRatePerSec, _uniffiThat.peerRatePerSec) == 0 && 
              
              java.lang.Float.compare(peerBurst, _uniffiThat.peerBurst) == 0 && 
              
              queueCapacity == _uniffiThat.queueCapacity && 
              
              java.lang.Float.compare(biasMinScale, _uniffiThat.biasMinScale) == 0 && 
              
              biasMaxHandicapMs == _uniffiThat.biasMaxHandicapMs && 
              
              activityWindowMs == _uniffiThat.activityWindowMs && 
              
              activityMinForwards == _uniffiThat.activityMinForwards && 
              
              activityIdleWindows == _uniffiThat.activityIdleWindows
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(maxTtl);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(denseMaxTtl);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(denseDegree);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(fanout);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(jitterMinMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(jitterMaxMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(ratePerSec);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(burst);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(peerRatePerSec);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(peerBurst);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(queueCapacity);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(biasMinScale);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(biasMaxHandicapMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(activityWindowMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(activityMinForwards);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(activityIdleWindows);
        return _uniffiHash;
    }

    
    
    

}


