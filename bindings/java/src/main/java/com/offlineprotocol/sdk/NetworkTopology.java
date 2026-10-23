package com.offlineprotocol.sdk;

public class NetworkTopology {
    private java.util.List<NetworkNode> nodes;
    private java.util.List<NetworkLink> links;
    private java.util.List<MessageStats> messageStats;

    public NetworkTopology(
        java.util.List<NetworkNode> nodes, 
        java.util.List<NetworkLink> links, 
        java.util.List<MessageStats> messageStats
    ) {
        
        this.nodes = nodes;
        
        this.links = links;
        
        this.messageStats = messageStats;
    }
    
    public java.util.List<NetworkNode> nodes() {
        return this.nodes;
    }
    
    public java.util.List<NetworkLink> links() {
        return this.links;
    }
    
    public java.util.List<MessageStats> messageStats() {
        return this.messageStats;
    }
    public void setNodes(java.util.List<NetworkNode> nodes) {
        this.nodes = nodes;
    }
    public void setLinks(java.util.List<NetworkLink> links) {
        this.links = links;
    }
    public void setMessageStats(java.util.List<MessageStats> messageStats) {
        this.messageStats = messageStats;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof NetworkTopology) {
            NetworkTopology _uniffiThat = (NetworkTopology) _uniffiOther;
            return (
              java.util.Objects.equals(nodes, _uniffiThat.nodes) && 
              
              java.util.Objects.equals(links, _uniffiThat.links) && 
              
              java.util.Objects.equals(messageStats, _uniffiThat.messageStats)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(nodes);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(links);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(messageStats);
        return _uniffiHash;
    }

    
    
    

}


