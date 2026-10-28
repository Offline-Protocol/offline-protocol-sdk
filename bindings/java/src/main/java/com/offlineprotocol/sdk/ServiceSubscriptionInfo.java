package com.offlineprotocol.sdk;

public class ServiceSubscriptionInfo {
    private java.lang.String subId;
    private java.lang.String provider;
    private java.lang.String serviceId;
    private java.lang.String topic;
    private SubscriptionMode mode;
    private java.lang.String state;

    public ServiceSubscriptionInfo(
        java.lang.String subId, 
        java.lang.String provider, 
        java.lang.String serviceId, 
        java.lang.String topic, 
        SubscriptionMode mode, 
        java.lang.String state
    ) {
        
        this.subId = subId;
        
        this.provider = provider;
        
        this.serviceId = serviceId;
        
        this.topic = topic;
        
        this.mode = mode;
        
        this.state = state;
    }
    
    public java.lang.String subId() {
        return this.subId;
    }
    
    public java.lang.String provider() {
        return this.provider;
    }
    
    public java.lang.String serviceId() {
        return this.serviceId;
    }
    
    public java.lang.String topic() {
        return this.topic;
    }
    
    public SubscriptionMode mode() {
        return this.mode;
    }
    
    public java.lang.String state() {
        return this.state;
    }
    public void setSubId(java.lang.String subId) {
        this.subId = subId;
    }
    public void setProvider(java.lang.String provider) {
        this.provider = provider;
    }
    public void setServiceId(java.lang.String serviceId) {
        this.serviceId = serviceId;
    }
    public void setTopic(java.lang.String topic) {
        this.topic = topic;
    }
    public void setMode(SubscriptionMode mode) {
        this.mode = mode;
    }
    public void setState(java.lang.String state) {
        this.state = state;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ServiceSubscriptionInfo) {
            ServiceSubscriptionInfo _uniffiThat = (ServiceSubscriptionInfo) _uniffiOther;
            return (
              java.util.Objects.equals(subId, _uniffiThat.subId) && 
              
              java.util.Objects.equals(provider, _uniffiThat.provider) && 
              
              java.util.Objects.equals(serviceId, _uniffiThat.serviceId) && 
              
              java.util.Objects.equals(topic, _uniffiThat.topic) && 
              
              java.util.Objects.equals(mode, _uniffiThat.mode) && 
              
              java.util.Objects.equals(state, _uniffiThat.state)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(subId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(provider);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(serviceId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(topic);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mode);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(state);
        return _uniffiHash;
    }

    
    
    

}


