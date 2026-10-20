package com.offlineprotocol.sdk;

public class RelayConfig {
    private byte minBatteryForRelay;
    private boolean allowRelay;
    private RelayPriority relayPriority;

    public RelayConfig(
        byte minBatteryForRelay, 
        boolean allowRelay, 
        RelayPriority relayPriority
    ) {
        
        this.minBatteryForRelay = minBatteryForRelay;
        
        this.allowRelay = allowRelay;
        
        this.relayPriority = relayPriority;
    }
    
    public byte minBatteryForRelay() {
        return this.minBatteryForRelay;
    }
    
    public boolean allowRelay() {
        return this.allowRelay;
    }
    
    public RelayPriority relayPriority() {
        return this.relayPriority;
    }
    public void setMinBatteryForRelay(byte minBatteryForRelay) {
        this.minBatteryForRelay = minBatteryForRelay;
    }
    public void setAllowRelay(boolean allowRelay) {
        this.allowRelay = allowRelay;
    }
    public void setRelayPriority(RelayPriority relayPriority) {
        this.relayPriority = relayPriority;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof RelayConfig) {
            RelayConfig _uniffiThat = (RelayConfig) _uniffiOther;
            return (
              minBatteryForRelay == _uniffiThat.minBatteryForRelay && 
              
              allowRelay == _uniffiThat.allowRelay && 
              
              java.util.Objects.equals(relayPriority, _uniffiThat.relayPriority)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(minBatteryForRelay);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(allowRelay);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(relayPriority);
        return _uniffiHash;
    }

    
    
    

}


