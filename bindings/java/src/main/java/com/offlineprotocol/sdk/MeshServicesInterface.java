package com.offlineprotocol.sdk;

public interface MeshServicesInterface {
    public void acceptServiceSubscription(java.lang.String subId) throws ProtocolException;
    public void clearServiceTopic(java.lang.String serviceId, java.lang.String topic) throws ProtocolException;
    public java.lang.String discoverServices(java.lang.String serviceId) throws ProtocolException;
    public void endServiceSubscription(java.lang.String subId, java.lang.String reason) throws ProtocolException;
    public ServicePublishReport publishServiceUpdate(java.lang.String serviceId, java.lang.String topic, java.lang.String key, java.lang.String body) throws ProtocolException;
    public void registerService(java.lang.String serviceId, java.lang.String version, java.util.Map<java.lang.String, java.lang.String> capabilities) throws ProtocolException;
    public void rejectServiceSubscription(java.lang.String subId, java.lang.String reason) throws ProtocolException;
    public java.lang.String respondToServiceRequest(java.lang.String requestId, java.lang.String requester, java.lang.String serviceId, java.lang.String status, java.lang.String body) throws ProtocolException;
    public void resyncServiceSubscription(java.lang.String subId) throws ProtocolException;
    public java.lang.String sendServiceRequest(java.lang.String provider, java.lang.String serviceId, java.lang.String method, java.lang.String body) throws ProtocolException;
    public java.util.List<ServiceSubscriberInfo> serviceSubscribers(java.lang.String serviceId) throws ProtocolException;
    public java.util.List<ServiceSubscriptionInfo> serviceSubscriptions() throws ProtocolException;
    public void setServiceSubscriptionPolicy(java.lang.String serviceId, SubscriptionPolicyConfig policy) throws ProtocolException;
    public void setServiceTopicCheckpoint(java.lang.String serviceId, java.lang.String topic, java.lang.String body) throws ProtocolException;
    public java.lang.String subscribeService(java.lang.String provider, java.lang.String serviceId, java.lang.String topic, SubscriptionMode mode, java.lang.String params) throws ProtocolException;
    public boolean unregisterService(java.lang.String serviceId) throws ProtocolException;
    public boolean unsubscribeService(java.lang.String subId) throws ProtocolException;
    
}

