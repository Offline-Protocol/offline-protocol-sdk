package com.offlineprotocol.sdk;

public class DedupStats {
    private long totalTracked;
    private long recentTracked;
    private byte capacityUsedPercent;
    private java.lang.String mode;

    public DedupStats(
        long totalTracked, 
        long recentTracked, 
        byte capacityUsedPercent, 
        java.lang.String mode
    ) {
        
        this.totalTracked = totalTracked;
        
        this.recentTracked = recentTracked;
        
        this.capacityUsedPercent = capacityUsedPercent;
        
        this.mode = mode;
    }
    
    public long totalTracked() {
        return this.totalTracked;
    }
    
    public long recentTracked() {
        return this.recentTracked;
    }
    
    public byte capacityUsedPercent() {
        return this.capacityUsedPercent;
    }
    
    public java.lang.String mode() {
        return this.mode;
    }
    public void setTotalTracked(long totalTracked) {
        this.totalTracked = totalTracked;
    }
    public void setRecentTracked(long recentTracked) {
        this.recentTracked = recentTracked;
    }
    public void setCapacityUsedPercent(byte capacityUsedPercent) {
        this.capacityUsedPercent = capacityUsedPercent;
    }
    public void setMode(java.lang.String mode) {
        this.mode = mode;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof DedupStats) {
            DedupStats _uniffiThat = (DedupStats) _uniffiOther;
            return (
              totalTracked == _uniffiThat.totalTracked && 
              
              recentTracked == _uniffiThat.recentTracked && 
              
              capacityUsedPercent == _uniffiThat.capacityUsedPercent && 
              
              java.util.Objects.equals(mode, _uniffiThat.mode)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(totalTracked);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(recentTracked);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(capacityUsedPercent);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mode);
        return _uniffiHash;
    }

    
    
    

}


