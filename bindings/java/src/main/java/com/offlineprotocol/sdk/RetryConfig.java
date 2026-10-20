package com.offlineprotocol.sdk;

public class RetryConfig {
    private int maxRetries;
    private long initialDelayMs;
    private long maxDelayMs;
    private float backoffMultiplier;
    private long outboxMaxLifetimeMs;
    private long pendingMessageMaxLifetimeMs;
    private boolean edgeDrivenUnreachableDm;

    public RetryConfig(
        int maxRetries, 
        long initialDelayMs, 
        long maxDelayMs, 
        float backoffMultiplier, 
        long outboxMaxLifetimeMs, 
        long pendingMessageMaxLifetimeMs, 
        boolean edgeDrivenUnreachableDm
    ) {
        
        this.maxRetries = maxRetries;
        
        this.initialDelayMs = initialDelayMs;
        
        this.maxDelayMs = maxDelayMs;
        
        this.backoffMultiplier = backoffMultiplier;
        
        this.outboxMaxLifetimeMs = outboxMaxLifetimeMs;
        
        this.pendingMessageMaxLifetimeMs = pendingMessageMaxLifetimeMs;
        
        this.edgeDrivenUnreachableDm = edgeDrivenUnreachableDm;
    }
    
    public int maxRetries() {
        return this.maxRetries;
    }
    
    public long initialDelayMs() {
        return this.initialDelayMs;
    }
    
    public long maxDelayMs() {
        return this.maxDelayMs;
    }
    
    public float backoffMultiplier() {
        return this.backoffMultiplier;
    }
    
    public long outboxMaxLifetimeMs() {
        return this.outboxMaxLifetimeMs;
    }
    
    public long pendingMessageMaxLifetimeMs() {
        return this.pendingMessageMaxLifetimeMs;
    }
    
    public boolean edgeDrivenUnreachableDm() {
        return this.edgeDrivenUnreachableDm;
    }
    public void setMaxRetries(int maxRetries) {
        this.maxRetries = maxRetries;
    }
    public void setInitialDelayMs(long initialDelayMs) {
        this.initialDelayMs = initialDelayMs;
    }
    public void setMaxDelayMs(long maxDelayMs) {
        this.maxDelayMs = maxDelayMs;
    }
    public void setBackoffMultiplier(float backoffMultiplier) {
        this.backoffMultiplier = backoffMultiplier;
    }
    public void setOutboxMaxLifetimeMs(long outboxMaxLifetimeMs) {
        this.outboxMaxLifetimeMs = outboxMaxLifetimeMs;
    }
    public void setPendingMessageMaxLifetimeMs(long pendingMessageMaxLifetimeMs) {
        this.pendingMessageMaxLifetimeMs = pendingMessageMaxLifetimeMs;
    }
    public void setEdgeDrivenUnreachableDm(boolean edgeDrivenUnreachableDm) {
        this.edgeDrivenUnreachableDm = edgeDrivenUnreachableDm;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof RetryConfig) {
            RetryConfig _uniffiThat = (RetryConfig) _uniffiOther;
            return (
              maxRetries == _uniffiThat.maxRetries && 
              
              initialDelayMs == _uniffiThat.initialDelayMs && 
              
              maxDelayMs == _uniffiThat.maxDelayMs && 
              
              java.lang.Float.compare(backoffMultiplier, _uniffiThat.backoffMultiplier) == 0 && 
              
              outboxMaxLifetimeMs == _uniffiThat.outboxMaxLifetimeMs && 
              
              pendingMessageMaxLifetimeMs == _uniffiThat.pendingMessageMaxLifetimeMs && 
              
              edgeDrivenUnreachableDm == _uniffiThat.edgeDrivenUnreachableDm
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(maxRetries);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(initialDelayMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxDelayMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(backoffMultiplier);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(outboxMaxLifetimeMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(pendingMessageMaxLifetimeMs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(edgeDrivenUnreachableDm);
        return _uniffiHash;
    }

    
    
    

}


