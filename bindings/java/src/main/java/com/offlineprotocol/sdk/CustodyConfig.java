package com.offlineprotocol.sdk;

public class CustodyConfig {
    private java.lang.Boolean enabled;
    private java.lang.Long holdMs;
    private java.lang.Long maxEntriesPerDepositor;
    private java.lang.Long maxBytesPerDepositor;
    private java.lang.Long maxEntries;
    private java.lang.Long maxBytes;
    private java.lang.Long strangerMaxEntries;
    private java.lang.Long strangerMaxBytes;
    private OverflowPolicy overflowPolicy;

    public CustodyConfig(
        java.lang.Boolean enabled, 
        java.lang.Long holdMs, 
        java.lang.Long maxEntriesPerDepositor, 
        java.lang.Long maxBytesPerDepositor, 
        java.lang.Long maxEntries, 
        java.lang.Long maxBytes, 
        java.lang.Long strangerMaxEntries, 
        java.lang.Long strangerMaxBytes, 
        OverflowPolicy overflowPolicy
    ) {
        
        this.enabled = enabled;
        
        this.holdMs = holdMs;
        
        this.maxEntriesPerDepositor = maxEntriesPerDepositor;
        
        this.maxBytesPerDepositor = maxBytesPerDepositor;
        
        this.maxEntries = maxEntries;
        
        this.maxBytes = maxBytes;
        
        this.strangerMaxEntries = strangerMaxEntries;
        
        this.strangerMaxBytes = strangerMaxBytes;
        
        this.overflowPolicy = overflowPolicy;
    }
    
    public java.lang.Boolean enabled() {
        return this.enabled;
    }
    
    public java.lang.Long holdMs() {
        return this.holdMs;
    }
    
    public java.lang.Long maxEntriesPerDepositor() {
        return this.maxEntriesPerDepositor;
    }
    
    public java.lang.Long maxBytesPerDepositor() {
        return this.maxBytesPerDepositor;
    }
    
    public java.lang.Long maxEntries() {
        return this.maxEntries;
    }
    
    public java.lang.Long maxBytes() {
        return this.maxBytes;
    }
    
    public java.lang.Long strangerMaxEntries() {
        return this.strangerMaxEntries;
    }
    
    public java.lang.Long strangerMaxBytes() {
        return this.strangerMaxBytes;
    }
    
    public OverflowPolicy overflowPolicy() {
        return this.overflowPolicy;
    }
    public void setEnabled(java.lang.Boolean enabled) {
        this.enabled = enabled;
    }
    public void setHoldMs(java.lang.Long holdMs) {
        this.holdMs = holdMs;
    }
    public void setMaxEntriesPerDepositor(java.lang.Long maxEntriesPerDepositor) {
        this.maxEntriesPerDepositor = maxEntriesPerDepositor;
    }
    public void setMaxBytesPerDepositor(java.lang.Long maxBytesPerDepositor) {
        this.maxBytesPerDepositor = maxBytesPerDepositor;
    }
    public void setMaxEntries(java.lang.Long maxEntries) {
        this.maxEntries = maxEntries;
    }
    public void setMaxBytes(java.lang.Long maxBytes) {
        this.maxBytes = maxBytes;
    }
    public void setStrangerMaxEntries(java.lang.Long strangerMaxEntries) {
        this.strangerMaxEntries = strangerMaxEntries;
    }
    public void setStrangerMaxBytes(java.lang.Long strangerMaxBytes) {
        this.strangerMaxBytes = strangerMaxBytes;
    }
    public void setOverflowPolicy(OverflowPolicy overflowPolicy) {
        this.overflowPolicy = overflowPolicy;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof CustodyConfig) {
            CustodyConfig _uniffiThat = (CustodyConfig) _uniffiOther;
            return (
              java.util.Objects.equals(enabled, _uniffiThat.enabled) && 
              
              java.util.Objects.equals(holdMs, _uniffiThat.holdMs) && 
              
              java.util.Objects.equals(maxEntriesPerDepositor, _uniffiThat.maxEntriesPerDepositor) && 
              
              java.util.Objects.equals(maxBytesPerDepositor, _uniffiThat.maxBytesPerDepositor) && 
              
              java.util.Objects.equals(maxEntries, _uniffiThat.maxEntries) && 
              
              java.util.Objects.equals(maxBytes, _uniffiThat.maxBytes) && 
              
              java.util.Objects.equals(strangerMaxEntries, _uniffiThat.strangerMaxEntries) && 
              
              java.util.Objects.equals(strangerMaxBytes, _uniffiThat.strangerMaxBytes) && 
              
              java.util.Objects.equals(overflowPolicy, _uniffiThat.overflowPolicy)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(enabled);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(holdMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxEntriesPerDepositor);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxBytesPerDepositor);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxEntries);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxBytes);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(strangerMaxEntries);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(strangerMaxBytes);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(overflowPolicy);
        return _uniffiHash;
    }

    
    
    

}


