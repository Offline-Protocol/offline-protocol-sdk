package com.offlineprotocol.sdk;

public interface MeshServicesInterface {
    public java.lang.String discoverServices(java.lang.String serviceId) throws ProtocolException;
    public void registerService(java.lang.String serviceId, java.lang.String version, java.util.Map<java.lang.String, java.lang.String> capabilities) throws ProtocolException;
    public java.lang.String respondToServiceRequest(java.lang.String requestId, java.lang.String requester, java.lang.String serviceId, java.lang.String status, java.lang.String body) throws ProtocolException;
    public java.lang.String sendServiceRequest(java.lang.String provider, java.lang.String serviceId, java.lang.String method, java.lang.String body) throws ProtocolException;
    public boolean unregisterService(java.lang.String serviceId) throws ProtocolException;
    
}

