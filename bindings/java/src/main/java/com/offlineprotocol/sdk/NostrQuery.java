package com.offlineprotocol.sdk;

public class NostrQuery {
    private java.lang.String queryId;
    private java.lang.String reqJson;

    public NostrQuery(
        java.lang.String queryId, 
        java.lang.String reqJson
    ) {
        
        this.queryId = queryId;
        
        this.reqJson = reqJson;
    }
    
    public java.lang.String queryId() {
        return this.queryId;
    }
    
    public java.lang.String reqJson() {
        return this.reqJson;
    }
    public void setQueryId(java.lang.String queryId) {
        this.queryId = queryId;
    }
    public void setReqJson(java.lang.String reqJson) {
        this.reqJson = reqJson;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof NostrQuery) {
            NostrQuery _uniffiThat = (NostrQuery) _uniffiOther;
            return (
              java.util.Objects.equals(queryId, _uniffiThat.queryId) && 
              
              java.util.Objects.equals(reqJson, _uniffiThat.reqJson)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(queryId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(reqJson);
        return _uniffiHash;
    }

    
    
    

}


