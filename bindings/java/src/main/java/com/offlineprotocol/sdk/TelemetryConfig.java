package com.offlineprotocol.sdk;

public class TelemetryConfig {
    private java.lang.String apiKey;
    private java.lang.String appId;
    private TelemetryOs os;
    private short osMajor;
    private java.lang.String appVersion;
    private java.lang.Boolean debug;
    private java.lang.Long flushIntervalMs;
    private java.lang.Long maxBatchBytes;
    private java.lang.Integer maxBufferedRecords;
    private java.lang.Boolean includeDeviceId;
    private java.lang.Boolean scrubIds;
    private MlsVerbosity mlsVerbosity;
    private java.lang.Long metricsCadenceMs;
    private java.lang.Boolean routingDiagnostic;
    private java.lang.Boolean mlsSamplingBypass;

    public TelemetryConfig(
        java.lang.String apiKey, 
        java.lang.String appId, 
        TelemetryOs os, 
        short osMajor, 
        java.lang.String appVersion, 
        java.lang.Boolean debug, 
        java.lang.Long flushIntervalMs, 
        java.lang.Long maxBatchBytes, 
        java.lang.Integer maxBufferedRecords, 
        java.lang.Boolean includeDeviceId, 
        java.lang.Boolean scrubIds, 
        MlsVerbosity mlsVerbosity, 
        java.lang.Long metricsCadenceMs, 
        java.lang.Boolean routingDiagnostic, 
        java.lang.Boolean mlsSamplingBypass
    ) {
        
        this.apiKey = apiKey;
        
        this.appId = appId;
        
        this.os = os;
        
        this.osMajor = osMajor;
        
        this.appVersion = appVersion;
        
        this.debug = debug;
        
        this.flushIntervalMs = flushIntervalMs;
        
        this.maxBatchBytes = maxBatchBytes;
        
        this.maxBufferedRecords = maxBufferedRecords;
        
        this.includeDeviceId = includeDeviceId;
        
        this.scrubIds = scrubIds;
        
        this.mlsVerbosity = mlsVerbosity;
        
        this.metricsCadenceMs = metricsCadenceMs;
        
        this.routingDiagnostic = routingDiagnostic;
        
        this.mlsSamplingBypass = mlsSamplingBypass;
    }
    
    public java.lang.String apiKey() {
        return this.apiKey;
    }
    
    public java.lang.String appId() {
        return this.appId;
    }
    
    public TelemetryOs os() {
        return this.os;
    }
    
    public short osMajor() {
        return this.osMajor;
    }
    
    public java.lang.String appVersion() {
        return this.appVersion;
    }
    
    public java.lang.Boolean debug() {
        return this.debug;
    }
    
    public java.lang.Long flushIntervalMs() {
        return this.flushIntervalMs;
    }
    
    public java.lang.Long maxBatchBytes() {
        return this.maxBatchBytes;
    }
    
    public java.lang.Integer maxBufferedRecords() {
        return this.maxBufferedRecords;
    }
    
    public java.lang.Boolean includeDeviceId() {
        return this.includeDeviceId;
    }
    
    public java.lang.Boolean scrubIds() {
        return this.scrubIds;
    }
    
    public MlsVerbosity mlsVerbosity() {
        return this.mlsVerbosity;
    }
    
    public java.lang.Long metricsCadenceMs() {
        return this.metricsCadenceMs;
    }
    
    public java.lang.Boolean routingDiagnostic() {
        return this.routingDiagnostic;
    }
    
    public java.lang.Boolean mlsSamplingBypass() {
        return this.mlsSamplingBypass;
    }
    public void setApiKey(java.lang.String apiKey) {
        this.apiKey = apiKey;
    }
    public void setAppId(java.lang.String appId) {
        this.appId = appId;
    }
    public void setOs(TelemetryOs os) {
        this.os = os;
    }
    public void setOsMajor(short osMajor) {
        this.osMajor = osMajor;
    }
    public void setAppVersion(java.lang.String appVersion) {
        this.appVersion = appVersion;
    }
    public void setDebug(java.lang.Boolean debug) {
        this.debug = debug;
    }
    public void setFlushIntervalMs(java.lang.Long flushIntervalMs) {
        this.flushIntervalMs = flushIntervalMs;
    }
    public void setMaxBatchBytes(java.lang.Long maxBatchBytes) {
        this.maxBatchBytes = maxBatchBytes;
    }
    public void setMaxBufferedRecords(java.lang.Integer maxBufferedRecords) {
        this.maxBufferedRecords = maxBufferedRecords;
    }
    public void setIncludeDeviceId(java.lang.Boolean includeDeviceId) {
        this.includeDeviceId = includeDeviceId;
    }
    public void setScrubIds(java.lang.Boolean scrubIds) {
        this.scrubIds = scrubIds;
    }
    public void setMlsVerbosity(MlsVerbosity mlsVerbosity) {
        this.mlsVerbosity = mlsVerbosity;
    }
    public void setMetricsCadenceMs(java.lang.Long metricsCadenceMs) {
        this.metricsCadenceMs = metricsCadenceMs;
    }
    public void setRoutingDiagnostic(java.lang.Boolean routingDiagnostic) {
        this.routingDiagnostic = routingDiagnostic;
    }
    public void setMlsSamplingBypass(java.lang.Boolean mlsSamplingBypass) {
        this.mlsSamplingBypass = mlsSamplingBypass;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof TelemetryConfig) {
            TelemetryConfig _uniffiThat = (TelemetryConfig) _uniffiOther;
            return (
              java.util.Objects.equals(apiKey, _uniffiThat.apiKey) && 
              
              java.util.Objects.equals(appId, _uniffiThat.appId) && 
              
              java.util.Objects.equals(os, _uniffiThat.os) && 
              
              osMajor == _uniffiThat.osMajor && 
              
              java.util.Objects.equals(appVersion, _uniffiThat.appVersion) && 
              
              java.util.Objects.equals(debug, _uniffiThat.debug) && 
              
              java.util.Objects.equals(flushIntervalMs, _uniffiThat.flushIntervalMs) && 
              
              java.util.Objects.equals(maxBatchBytes, _uniffiThat.maxBatchBytes) && 
              
              java.util.Objects.equals(maxBufferedRecords, _uniffiThat.maxBufferedRecords) && 
              
              java.util.Objects.equals(includeDeviceId, _uniffiThat.includeDeviceId) && 
              
              java.util.Objects.equals(scrubIds, _uniffiThat.scrubIds) && 
              
              java.util.Objects.equals(mlsVerbosity, _uniffiThat.mlsVerbosity) && 
              
              java.util.Objects.equals(metricsCadenceMs, _uniffiThat.metricsCadenceMs) && 
              
              java.util.Objects.equals(routingDiagnostic, _uniffiThat.routingDiagnostic) && 
              
              java.util.Objects.equals(mlsSamplingBypass, _uniffiThat.mlsSamplingBypass)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(apiKey);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(appId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(os);
        _uniffiHash = 31 * _uniffiHash + java.lang.Short.hashCode(osMajor);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(appVersion);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(debug);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(flushIntervalMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxBatchBytes);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(maxBufferedRecords);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(includeDeviceId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(scrubIds);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mlsVerbosity);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(metricsCadenceMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(routingDiagnostic);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mlsSamplingBypass);
        return _uniffiHash;
    }

    
    
    

}


