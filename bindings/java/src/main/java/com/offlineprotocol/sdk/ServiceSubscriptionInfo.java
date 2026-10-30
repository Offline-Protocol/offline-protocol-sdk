package com.offlineprotocol.sdk;

public class ServiceSubscriptionInfo {
    private java.lang.String subId;
    private java.lang.String provider;
    private java.lang.String serviceId;
    private java.lang.String topic;
    private SubscriptionMode mode;
    private SubscriptionState state;
    private java.util.List<java.lang.String> keys;
    private java.util.List<java.lang.String> topics;

    public ServiceSubscriptionInfo(
        java.lang.String subId, 
        java.lang.String provider, 
        java.lang.String serviceId, 
        java.lang.String topic, 
        SubscriptionMode mode, 
        SubscriptionState state, 
        java.util.List<java.lang.String> keys, 
        java.util.List<java.lang.String> topics
    ) {
        
        this.subId = subId;
        
        this.provider = provider;
        
        this.serviceId = serviceId;
        
        this.topic = topic;
        
        this.mode = mode;
        
        this.state = state;
        
        this.keys = keys;
        
        this.topics = topics;
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
    
    public SubscriptionState state() {
        return this.state;
    }
    
    public java.util.List<java.lang.String> keys() {
        return this.keys;
    }
    
    public java.util.List<java.lang.String> topics() {
        return this.topics;
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
    public void setState(SubscriptionState state) {
        this.state = state;
    }
    public void setKeys(java.util.List<java.lang.String> keys) {
        this.keys = keys;
    }
    public void setTopics(java.util.List<java.lang.String> topics) {
        this.topics = topics;
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
              
              java.util.Objects.equals(state, _uniffiThat.state) && 
              
              java.util.Objects.equals(keys, _uniffiThat.keys) && 
              
              java.util.Objects.equals(topics, _uniffiThat.topics)
              
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
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(keys);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(topics);
        return _uniffiHash;
    }

    
    
    

}


