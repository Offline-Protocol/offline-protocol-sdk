package com.offlineprotocol.sdk;

public class ProtocolLockDiagnostics {
    private boolean held;
    private java.lang.String holderLocation;
    private java.lang.String holderThread;
    private long heldForMs;

    public ProtocolLockDiagnostics(
        boolean held, 
        java.lang.String holderLocation, 
        java.lang.String holderThread, 
        long heldForMs
    ) {
        
        this.held = held;
        
        this.holderLocation = holderLocation;
        
        this.holderThread = holderThread;
        
        this.heldForMs = heldForMs;
    }
    
    public boolean held() {
        return this.held;
    }
    
    public java.lang.String holderLocation() {
        return this.holderLocation;
    }
    
    public java.lang.String holderThread() {
        return this.holderThread;
    }
    
    public long heldForMs() {
        return this.heldForMs;
    }
    public void setHeld(boolean held) {
        this.held = held;
    }
    public void setHolderLocation(java.lang.String holderLocation) {
        this.holderLocation = holderLocation;
    }
    public void setHolderThread(java.lang.String holderThread) {
        this.holderThread = holderThread;
    }
    public void setHeldForMs(long heldForMs) {
        this.heldForMs = heldForMs;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ProtocolLockDiagnostics) {
            ProtocolLockDiagnostics _uniffiThat = (ProtocolLockDiagnostics) _uniffiOther;
            return (
              held == _uniffiThat.held && 
              
              java.util.Objects.equals(holderLocation, _uniffiThat.holderLocation) && 
              
              java.util.Objects.equals(holderThread, _uniffiThat.holderThread) && 
              
              heldForMs == _uniffiThat.heldForMs
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(held);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(holderLocation);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(holderThread);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(heldForMs);
        return _uniffiHash;
    }

    
    
    

}


