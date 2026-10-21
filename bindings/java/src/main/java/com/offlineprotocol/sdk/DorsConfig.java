package com.offlineprotocol.sdk;

public class DorsConfig {
    private boolean preferOnline;
    private float switchHysteresis;
    private long switchCooldownSecs;
    private int bleToWifiRetryThreshold;
    private float minSuccessRateBeforeEscalation;
    private long minBleSamplesBeforeSuccessRateEscalation;
    private short rssiSwitchThreshold;
    private long congestionQueueThreshold;
    private long stabilityWindowSecs;
    private long poorSignalDurationSecs;
    private byte ttlEscalationThreshold;
    private long congestionDurationSecs;
    private long ttlEscalationHoldSecs;
    private long historyWindowSize;
    private float queueRecoveryRatio;
    private byte lowBatteryThreshold;
    private byte relayMinBatteryLevel;
    private byte relayOptimalConnectionCount;

    public DorsConfig(
        boolean preferOnline, 
        float switchHysteresis, 
        long switchCooldownSecs, 
        int bleToWifiRetryThreshold, 
        float minSuccessRateBeforeEscalation, 
        long minBleSamplesBeforeSuccessRateEscalation, 
        short rssiSwitchThreshold, 
        long congestionQueueThreshold, 
        long stabilityWindowSecs, 
        long poorSignalDurationSecs, 
        byte ttlEscalationThreshold, 
        long congestionDurationSecs, 
        long ttlEscalationHoldSecs, 
        long historyWindowSize, 
        float queueRecoveryRatio, 
        byte lowBatteryThreshold, 
        byte relayMinBatteryLevel, 
        byte relayOptimalConnectionCount
    ) {
        
        this.preferOnline = preferOnline;
        
        this.switchHysteresis = switchHysteresis;
        
        this.switchCooldownSecs = switchCooldownSecs;
        
        this.bleToWifiRetryThreshold = bleToWifiRetryThreshold;
        
        this.minSuccessRateBeforeEscalation = minSuccessRateBeforeEscalation;
        
        this.minBleSamplesBeforeSuccessRateEscalation = minBleSamplesBeforeSuccessRateEscalation;
        
        this.rssiSwitchThreshold = rssiSwitchThreshold;
        
        this.congestionQueueThreshold = congestionQueueThreshold;
        
        this.stabilityWindowSecs = stabilityWindowSecs;
        
        this.poorSignalDurationSecs = poorSignalDurationSecs;
        
        this.ttlEscalationThreshold = ttlEscalationThreshold;
        
        this.congestionDurationSecs = congestionDurationSecs;
        
        this.ttlEscalationHoldSecs = ttlEscalationHoldSecs;
        
        this.historyWindowSize = historyWindowSize;
        
        this.queueRecoveryRatio = queueRecoveryRatio;
        
        this.lowBatteryThreshold = lowBatteryThreshold;
        
        this.relayMinBatteryLevel = relayMinBatteryLevel;
        
        this.relayOptimalConnectionCount = relayOptimalConnectionCount;
    }
    
    public boolean preferOnline() {
        return this.preferOnline;
    }
    
    public float switchHysteresis() {
        return this.switchHysteresis;
    }
    
    public long switchCooldownSecs() {
        return this.switchCooldownSecs;
    }
    
    public int bleToWifiRetryThreshold() {
        return this.bleToWifiRetryThreshold;
    }
    
    public float minSuccessRateBeforeEscalation() {
        return this.minSuccessRateBeforeEscalation;
    }
    
    public long minBleSamplesBeforeSuccessRateEscalation() {
        return this.minBleSamplesBeforeSuccessRateEscalation;
    }
    
    public short rssiSwitchThreshold() {
        return this.rssiSwitchThreshold;
    }
    
    public long congestionQueueThreshold() {
        return this.congestionQueueThreshold;
    }
    
    public long stabilityWindowSecs() {
        return this.stabilityWindowSecs;
    }
    
    public long poorSignalDurationSecs() {
        return this.poorSignalDurationSecs;
    }
    
    public byte ttlEscalationThreshold() {
        return this.ttlEscalationThreshold;
    }
    
    public long congestionDurationSecs() {
        return this.congestionDurationSecs;
    }
    
    public long ttlEscalationHoldSecs() {
        return this.ttlEscalationHoldSecs;
    }
    
    public long historyWindowSize() {
        return this.historyWindowSize;
    }
    
    public float queueRecoveryRatio() {
        return this.queueRecoveryRatio;
    }
    
    public byte lowBatteryThreshold() {
        return this.lowBatteryThreshold;
    }
    
    public byte relayMinBatteryLevel() {
        return this.relayMinBatteryLevel;
    }
    
    public byte relayOptimalConnectionCount() {
        return this.relayOptimalConnectionCount;
    }
    public void setPreferOnline(boolean preferOnline) {
        this.preferOnline = preferOnline;
    }
    public void setSwitchHysteresis(float switchHysteresis) {
        this.switchHysteresis = switchHysteresis;
    }
    public void setSwitchCooldownSecs(long switchCooldownSecs) {
        this.switchCooldownSecs = switchCooldownSecs;
    }
    public void setBleToWifiRetryThreshold(int bleToWifiRetryThreshold) {
        this.bleToWifiRetryThreshold = bleToWifiRetryThreshold;
    }
    public void setMinSuccessRateBeforeEscalation(float minSuccessRateBeforeEscalation) {
        this.minSuccessRateBeforeEscalation = minSuccessRateBeforeEscalation;
    }
    public void setMinBleSamplesBeforeSuccessRateEscalation(long minBleSamplesBeforeSuccessRateEscalation) {
        this.minBleSamplesBeforeSuccessRateEscalation = minBleSamplesBeforeSuccessRateEscalation;
    }
    public void setRssiSwitchThreshold(short rssiSwitchThreshold) {
        this.rssiSwitchThreshold = rssiSwitchThreshold;
    }
    public void setCongestionQueueThreshold(long congestionQueueThreshold) {
        this.congestionQueueThreshold = congestionQueueThreshold;
    }
    public void setStabilityWindowSecs(long stabilityWindowSecs) {
        this.stabilityWindowSecs = stabilityWindowSecs;
    }
    public void setPoorSignalDurationSecs(long poorSignalDurationSecs) {
        this.poorSignalDurationSecs = poorSignalDurationSecs;
    }
    public void setTtlEscalationThreshold(byte ttlEscalationThreshold) {
        this.ttlEscalationThreshold = ttlEscalationThreshold;
    }
    public void setCongestionDurationSecs(long congestionDurationSecs) {
        this.congestionDurationSecs = congestionDurationSecs;
    }
    public void setTtlEscalationHoldSecs(long ttlEscalationHoldSecs) {
        this.ttlEscalationHoldSecs = ttlEscalationHoldSecs;
    }
    public void setHistoryWindowSize(long historyWindowSize) {
        this.historyWindowSize = historyWindowSize;
    }
    public void setQueueRecoveryRatio(float queueRecoveryRatio) {
        this.queueRecoveryRatio = queueRecoveryRatio;
    }
    public void setLowBatteryThreshold(byte lowBatteryThreshold) {
        this.lowBatteryThreshold = lowBatteryThreshold;
    }
    public void setRelayMinBatteryLevel(byte relayMinBatteryLevel) {
        this.relayMinBatteryLevel = relayMinBatteryLevel;
    }
    public void setRelayOptimalConnectionCount(byte relayOptimalConnectionCount) {
        this.relayOptimalConnectionCount = relayOptimalConnectionCount;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof DorsConfig) {
            DorsConfig _uniffiThat = (DorsConfig) _uniffiOther;
            return (
              preferOnline == _uniffiThat.preferOnline && 
              
              java.lang.Float.compare(switchHysteresis, _uniffiThat.switchHysteresis) == 0 && 
              
              switchCooldownSecs == _uniffiThat.switchCooldownSecs && 
              
              bleToWifiRetryThreshold == _uniffiThat.bleToWifiRetryThreshold && 
              
              java.lang.Float.compare(minSuccessRateBeforeEscalation, _uniffiThat.minSuccessRateBeforeEscalation) == 0 && 
              
              minBleSamplesBeforeSuccessRateEscalation == _uniffiThat.minBleSamplesBeforeSuccessRateEscalation && 
              
              rssiSwitchThreshold == _uniffiThat.rssiSwitchThreshold && 
              
              congestionQueueThreshold == _uniffiThat.congestionQueueThreshold && 
              
              stabilityWindowSecs == _uniffiThat.stabilityWindowSecs && 
              
              poorSignalDurationSecs == _uniffiThat.poorSignalDurationSecs && 
              
              ttlEscalationThreshold == _uniffiThat.ttlEscalationThreshold && 
              
              congestionDurationSecs == _uniffiThat.congestionDurationSecs && 
              
              ttlEscalationHoldSecs == _uniffiThat.ttlEscalationHoldSecs && 
              
              historyWindowSize == _uniffiThat.historyWindowSize && 
              
              java.lang.Float.compare(queueRecoveryRatio, _uniffiThat.queueRecoveryRatio) == 0 && 
              
              lowBatteryThreshold == _uniffiThat.lowBatteryThreshold && 
              
              relayMinBatteryLevel == _uniffiThat.relayMinBatteryLevel && 
              
              relayOptimalConnectionCount == _uniffiThat.relayOptimalConnectionCount
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(preferOnline);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(switchHysteresis);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(switchCooldownSecs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(bleToWifiRetryThreshold);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(minSuccessRateBeforeEscalation);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(minBleSamplesBeforeSuccessRateEscalation);
        _uniffiHash = 31 * _uniffiHash + java.lang.Short.hashCode(rssiSwitchThreshold);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(congestionQueueThreshold);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(stabilityWindowSecs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(poorSignalDurationSecs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(ttlEscalationThreshold);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(congestionDurationSecs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(ttlEscalationHoldSecs);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(historyWindowSize);
        _uniffiHash = 31 * _uniffiHash + java.lang.Float.hashCode(queueRecoveryRatio);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(lowBatteryThreshold);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(relayMinBatteryLevel);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(relayOptimalConnectionCount);
        return _uniffiHash;
    }

    
    
    

}


