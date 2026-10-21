package com.offlineprotocol.sdk;

public class EncryptionConfig {
    private boolean enabled;
    private boolean autoKeyExchange;
    private boolean storePending;
    private boolean requireEncryption;
    private PendingQueueConfig pendingQueue;

    public EncryptionConfig(
        boolean enabled, 
        boolean autoKeyExchange, 
        boolean storePending, 
        boolean requireEncryption, 
        PendingQueueConfig pendingQueue
    ) {
        
        this.enabled = enabled;
        
        this.autoKeyExchange = autoKeyExchange;
        
        this.storePending = storePending;
        
        this.requireEncryption = requireEncryption;
        
        this.pendingQueue = pendingQueue;
    }
    
    public boolean enabled() {
        return this.enabled;
    }
    
    public boolean autoKeyExchange() {
        return this.autoKeyExchange;
    }
    
    public boolean storePending() {
        return this.storePending;
    }
    
    public boolean requireEncryption() {
        return this.requireEncryption;
    }
    
    public PendingQueueConfig pendingQueue() {
        return this.pendingQueue;
    }
    public void setEnabled(boolean enabled) {
        this.enabled = enabled;
    }
    public void setAutoKeyExchange(boolean autoKeyExchange) {
        this.autoKeyExchange = autoKeyExchange;
    }
    public void setStorePending(boolean storePending) {
        this.storePending = storePending;
    }
    public void setRequireEncryption(boolean requireEncryption) {
        this.requireEncryption = requireEncryption;
    }
    public void setPendingQueue(PendingQueueConfig pendingQueue) {
        this.pendingQueue = pendingQueue;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof EncryptionConfig) {
            EncryptionConfig _uniffiThat = (EncryptionConfig) _uniffiOther;
            return (
              enabled == _uniffiThat.enabled && 
              
              autoKeyExchange == _uniffiThat.autoKeyExchange && 
              
              storePending == _uniffiThat.storePending && 
              
              requireEncryption == _uniffiThat.requireEncryption && 
              
              java.util.Objects.equals(pendingQueue, _uniffiThat.pendingQueue)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(enabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(autoKeyExchange);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(storePending);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(requireEncryption);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(pendingQueue);
        return _uniffiHash;
    }

    
    
    

}


