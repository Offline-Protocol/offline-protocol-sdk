package com.offlineprotocol.sdk;

public class SubscriptionPolicyConfig {
    private SubscriptionPolicyKind kind;
    private java.util.List<java.lang.String> allowPeers;
    private java.util.List<java.lang.String> allowGroups;
    private SubscriptionRulesFallback rulesFallback;

    public SubscriptionPolicyConfig(
        SubscriptionPolicyKind kind, 
        java.util.List<java.lang.String> allowPeers, 
        java.util.List<java.lang.String> allowGroups, 
        SubscriptionRulesFallback rulesFallback
    ) {
        
        this.kind = kind;
        
        this.allowPeers = allowPeers;
        
        this.allowGroups = allowGroups;
        
        this.rulesFallback = rulesFallback;
    }
    
    public SubscriptionPolicyKind kind() {
        return this.kind;
    }
    
    public java.util.List<java.lang.String> allowPeers() {
        return this.allowPeers;
    }
    
    public java.util.List<java.lang.String> allowGroups() {
        return this.allowGroups;
    }
    
    public SubscriptionRulesFallback rulesFallback() {
        return this.rulesFallback;
    }
    public void setKind(SubscriptionPolicyKind kind) {
        this.kind = kind;
    }
    public void setAllowPeers(java.util.List<java.lang.String> allowPeers) {
        this.allowPeers = allowPeers;
    }
    public void setAllowGroups(java.util.List<java.lang.String> allowGroups) {
        this.allowGroups = allowGroups;
    }
    public void setRulesFallback(SubscriptionRulesFallback rulesFallback) {
        this.rulesFallback = rulesFallback;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof SubscriptionPolicyConfig) {
            SubscriptionPolicyConfig _uniffiThat = (SubscriptionPolicyConfig) _uniffiOther;
            return (
              java.util.Objects.equals(kind, _uniffiThat.kind) && 
              
              java.util.Objects.equals(allowPeers, _uniffiThat.allowPeers) && 
              
              java.util.Objects.equals(allowGroups, _uniffiThat.allowGroups) && 
              
              java.util.Objects.equals(rulesFallback, _uniffiThat.rulesFallback)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(kind);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(allowPeers);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(allowGroups);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(rulesFallback);
        return _uniffiHash;
    }

    
    
    

}


