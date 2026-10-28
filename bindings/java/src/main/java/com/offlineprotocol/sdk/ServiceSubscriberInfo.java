package com.offlineprotocol.sdk;

public class ServiceSubscriberInfo {
    private java.lang.String subId;
    private java.lang.String subscriber;
    private java.lang.String serviceId;
    private java.lang.String topic;
    private SubscriptionMode mode;
    private java.lang.String state;
    private java.lang.String params;
    private java.util.List<java.lang.String> keys;

    public ServiceSubscriberInfo(
        java.lang.String subId, 
        java.lang.String subscriber, 
        java.lang.String serviceId, 
        java.lang.String topic, 
        SubscriptionMode mode, 
        java.lang.String state, 
        java.lang.String params, 
        java.util.List<java.lang.String> keys
    ) {
        
        this.subId = subId;
        
        this.subscriber = subscriber;
        
        this.serviceId = serviceId;
        
        this.topic = topic;
        
        this.mode = mode;
        
        this.state = state;
        
        this.params = params;
        
        this.keys = keys;
    }
    
    public java.lang.String subId() {
        return this.subId;
    }
    
    public java.lang.String subscriber() {
        return this.subscriber;
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
    
    public java.lang.String params() {
        return this.params;
    }
    
    public java.util.List<java.lang.String> keys() {
        return this.keys;
    }
    public void setSubId(java.lang.String subId) {
        this.subId = subId;
    }
    public void setSubscriber(java.lang.String subscriber) {
        this.subscriber = subscriber;
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
    public void setParams(java.lang.String params) {
        this.params = params;
    }
    public void setKeys(java.util.List<java.lang.String> keys) {
        this.keys = keys;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ServiceSubscriberInfo) {
            ServiceSubscriberInfo _uniffiThat = (ServiceSubscriberInfo) _uniffiOther;
            return (
              java.util.Objects.equals(subId, _uniffiThat.subId) && 
              
              java.util.Objects.equals(subscriber, _uniffiThat.subscriber) && 
              
              java.util.Objects.equals(serviceId, _uniffiThat.serviceId) && 
              
              java.util.Objects.equals(topic, _uniffiThat.topic) && 
              
              java.util.Objects.equals(mode, _uniffiThat.mode) && 
              
              java.util.Objects.equals(state, _uniffiThat.state) && 
              
              java.util.Objects.equals(params, _uniffiThat.params) && 
              
              java.util.Objects.equals(keys, _uniffiThat.keys)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(subId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(subscriber);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(serviceId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(topic);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mode);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(state);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(params);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(keys);
        return _uniffiHash;
    }

    
    
    

}


