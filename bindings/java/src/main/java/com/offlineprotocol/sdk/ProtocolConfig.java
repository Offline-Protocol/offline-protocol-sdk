package com.offlineprotocol.sdk;

public class ProtocolConfig {
    private java.lang.String appId;
    private java.lang.String profile;
    private boolean bleEnabled;
    private boolean wifiDirectEnabled;
    private boolean internetEnabled;
    private boolean reticulumEnabled;
    private boolean nostrEnabled;
    private boolean preferOnline;
    private byte initialTtl;
    private boolean encryptionEnabled;
    private boolean autoKeyExchange;
    private boolean storePending;
    private boolean requireEncryption;
    private long maxPendingPerPeer;
    private long maxPendingGlobal;
    private long pendingTtlMs;
    private OverflowPolicy overflowPolicy;
    private boolean edgeDrivenUnreachableDm;
    private int maxGroupMembers;
    private boolean groupRelayEnabled;
    private boolean groupRelayBroadcastEnabled;
    private boolean groupEnforceAdminCommits;
    private boolean requireTransportIdentity;
    private boolean binaryWireEnabled;
    private boolean nostrSealingEnabled;
    private boolean nostrColdContactEnabled;
    private boolean nostrUsernameDiscoveryEnabled;
    private boolean compactEnvelopeEnabled;
    private boolean richPayloadEnabled;
    private boolean cryptoRecoveryEnabled;
    private MeshRelayConfig meshRelay;
    private CustodyConfig custody;
    private boolean dataEnabled;
    private boolean controlFreshnessEnforced;
    private boolean encryptServiceMessages;

    public ProtocolConfig(
        java.lang.String appId, 
        java.lang.String profile, 
        boolean bleEnabled, 
        boolean wifiDirectEnabled, 
        boolean internetEnabled, 
        boolean reticulumEnabled, 
        boolean nostrEnabled, 
        boolean preferOnline, 
        byte initialTtl, 
        boolean encryptionEnabled, 
        boolean autoKeyExchange, 
        boolean storePending, 
        boolean requireEncryption, 
        long maxPendingPerPeer, 
        long maxPendingGlobal, 
        long pendingTtlMs, 
        OverflowPolicy overflowPolicy, 
        boolean edgeDrivenUnreachableDm, 
        int maxGroupMembers, 
        boolean groupRelayEnabled, 
        boolean groupRelayBroadcastEnabled, 
        boolean groupEnforceAdminCommits, 
        boolean requireTransportIdentity, 
        boolean binaryWireEnabled, 
        boolean nostrSealingEnabled, 
        boolean nostrColdContactEnabled, 
        boolean nostrUsernameDiscoveryEnabled, 
        boolean compactEnvelopeEnabled, 
        boolean richPayloadEnabled, 
        boolean cryptoRecoveryEnabled, 
        MeshRelayConfig meshRelay, 
        CustodyConfig custody, 
        boolean dataEnabled, 
        boolean controlFreshnessEnforced, 
        boolean encryptServiceMessages
    ) {
        
        this.appId = appId;
        
        this.profile = profile;
        
        this.bleEnabled = bleEnabled;
        
        this.wifiDirectEnabled = wifiDirectEnabled;
        
        this.internetEnabled = internetEnabled;
        
        this.reticulumEnabled = reticulumEnabled;
        
        this.nostrEnabled = nostrEnabled;
        
        this.preferOnline = preferOnline;
        
        this.initialTtl = initialTtl;
        
        this.encryptionEnabled = encryptionEnabled;
        
        this.autoKeyExchange = autoKeyExchange;
        
        this.storePending = storePending;
        
        this.requireEncryption = requireEncryption;
        
        this.maxPendingPerPeer = maxPendingPerPeer;
        
        this.maxPendingGlobal = maxPendingGlobal;
        
        this.pendingTtlMs = pendingTtlMs;
        
        this.overflowPolicy = overflowPolicy;
        
        this.edgeDrivenUnreachableDm = edgeDrivenUnreachableDm;
        
        this.maxGroupMembers = maxGroupMembers;
        
        this.groupRelayEnabled = groupRelayEnabled;
        
        this.groupRelayBroadcastEnabled = groupRelayBroadcastEnabled;
        
        this.groupEnforceAdminCommits = groupEnforceAdminCommits;
        
        this.requireTransportIdentity = requireTransportIdentity;
        
        this.binaryWireEnabled = binaryWireEnabled;
        
        this.nostrSealingEnabled = nostrSealingEnabled;
        
        this.nostrColdContactEnabled = nostrColdContactEnabled;
        
        this.nostrUsernameDiscoveryEnabled = nostrUsernameDiscoveryEnabled;
        
        this.compactEnvelopeEnabled = compactEnvelopeEnabled;
        
        this.richPayloadEnabled = richPayloadEnabled;
        
        this.cryptoRecoveryEnabled = cryptoRecoveryEnabled;
        
        this.meshRelay = meshRelay;
        
        this.custody = custody;
        
        this.dataEnabled = dataEnabled;
        
        this.controlFreshnessEnforced = controlFreshnessEnforced;
        
        this.encryptServiceMessages = encryptServiceMessages;
    }
    
    public java.lang.String appId() {
        return this.appId;
    }
    
    public java.lang.String profile() {
        return this.profile;
    }
    
    public boolean bleEnabled() {
        return this.bleEnabled;
    }
    
    public boolean wifiDirectEnabled() {
        return this.wifiDirectEnabled;
    }
    
    public boolean internetEnabled() {
        return this.internetEnabled;
    }
    
    public boolean reticulumEnabled() {
        return this.reticulumEnabled;
    }
    
    public boolean nostrEnabled() {
        return this.nostrEnabled;
    }
    
    public boolean preferOnline() {
        return this.preferOnline;
    }
    
    public byte initialTtl() {
        return this.initialTtl;
    }
    
    public boolean encryptionEnabled() {
        return this.encryptionEnabled;
    }
    
    public boolean autoKeyExchange() {
        return this.autoKeyExchange;
    }
    
    public boolean storePending() {
        return this.storePending;
    }
    
    public boolean requireEncryption() {
        return this.requireEncryption;
    }
    
    public long maxPendingPerPeer() {
        return this.maxPendingPerPeer;
    }
    
    public long maxPendingGlobal() {
        return this.maxPendingGlobal;
    }
    
    public long pendingTtlMs() {
        return this.pendingTtlMs;
    }
    
    public OverflowPolicy overflowPolicy() {
        return this.overflowPolicy;
    }
    
    public boolean edgeDrivenUnreachableDm() {
        return this.edgeDrivenUnreachableDm;
    }
    
    public int maxGroupMembers() {
        return this.maxGroupMembers;
    }
    
    public boolean groupRelayEnabled() {
        return this.groupRelayEnabled;
    }
    
    public boolean groupRelayBroadcastEnabled() {
        return this.groupRelayBroadcastEnabled;
    }
    
    public boolean groupEnforceAdminCommits() {
        return this.groupEnforceAdminCommits;
    }
    
    public boolean requireTransportIdentity() {
        return this.requireTransportIdentity;
    }
    
    public boolean binaryWireEnabled() {
        return this.binaryWireEnabled;
    }
    
    public boolean nostrSealingEnabled() {
        return this.nostrSealingEnabled;
    }
    
    public boolean nostrColdContactEnabled() {
        return this.nostrColdContactEnabled;
    }
    
    public boolean nostrUsernameDiscoveryEnabled() {
        return this.nostrUsernameDiscoveryEnabled;
    }
    
    public boolean compactEnvelopeEnabled() {
        return this.compactEnvelopeEnabled;
    }
    
    public boolean richPayloadEnabled() {
        return this.richPayloadEnabled;
    }
    
    public boolean cryptoRecoveryEnabled() {
        return this.cryptoRecoveryEnabled;
    }
    
    public MeshRelayConfig meshRelay() {
        return this.meshRelay;
    }
    
    public CustodyConfig custody() {
        return this.custody;
    }
    
    public boolean dataEnabled() {
        return this.dataEnabled;
    }
    
    public boolean controlFreshnessEnforced() {
        return this.controlFreshnessEnforced;
    }
    
    public boolean encryptServiceMessages() {
        return this.encryptServiceMessages;
    }
    public void setAppId(java.lang.String appId) {
        this.appId = appId;
    }
    public void setProfile(java.lang.String profile) {
        this.profile = profile;
    }
    public void setBleEnabled(boolean bleEnabled) {
        this.bleEnabled = bleEnabled;
    }
    public void setWifiDirectEnabled(boolean wifiDirectEnabled) {
        this.wifiDirectEnabled = wifiDirectEnabled;
    }
    public void setInternetEnabled(boolean internetEnabled) {
        this.internetEnabled = internetEnabled;
    }
    public void setReticulumEnabled(boolean reticulumEnabled) {
        this.reticulumEnabled = reticulumEnabled;
    }
    public void setNostrEnabled(boolean nostrEnabled) {
        this.nostrEnabled = nostrEnabled;
    }
    public void setPreferOnline(boolean preferOnline) {
        this.preferOnline = preferOnline;
    }
    public void setInitialTtl(byte initialTtl) {
        this.initialTtl = initialTtl;
    }
    public void setEncryptionEnabled(boolean encryptionEnabled) {
        this.encryptionEnabled = encryptionEnabled;
    }
    public void setAutoKeyExchange(boolean autoKeyExchange) {
        this.autoKeyExchange = autoKeyExchange;
    }
    public void setStorePending(boolean storePending) {
        this.storePending = storePending;
    }
    public void setRequireEncryption(boolean requireEncryption) {
        this.requireEncryption = requireEncryption;
    }
    public void setMaxPendingPerPeer(long maxPendingPerPeer) {
        this.maxPendingPerPeer = maxPendingPerPeer;
    }
    public void setMaxPendingGlobal(long maxPendingGlobal) {
        this.maxPendingGlobal = maxPendingGlobal;
    }
    public void setPendingTtlMs(long pendingTtlMs) {
        this.pendingTtlMs = pendingTtlMs;
    }
    public void setOverflowPolicy(OverflowPolicy overflowPolicy) {
        this.overflowPolicy = overflowPolicy;
    }
    public void setEdgeDrivenUnreachableDm(boolean edgeDrivenUnreachableDm) {
        this.edgeDrivenUnreachableDm = edgeDrivenUnreachableDm;
    }
    public void setMaxGroupMembers(int maxGroupMembers) {
        this.maxGroupMembers = maxGroupMembers;
    }
    public void setGroupRelayEnabled(boolean groupRelayEnabled) {
        this.groupRelayEnabled = groupRelayEnabled;
    }
    public void setGroupRelayBroadcastEnabled(boolean groupRelayBroadcastEnabled) {
        this.groupRelayBroadcastEnabled = groupRelayBroadcastEnabled;
    }
    public void setGroupEnforceAdminCommits(boolean groupEnforceAdminCommits) {
        this.groupEnforceAdminCommits = groupEnforceAdminCommits;
    }
    public void setRequireTransportIdentity(boolean requireTransportIdentity) {
        this.requireTransportIdentity = requireTransportIdentity;
    }
    public void setBinaryWireEnabled(boolean binaryWireEnabled) {
        this.binaryWireEnabled = binaryWireEnabled;
    }
    public void setNostrSealingEnabled(boolean nostrSealingEnabled) {
        this.nostrSealingEnabled = nostrSealingEnabled;
    }
    public void setNostrColdContactEnabled(boolean nostrColdContactEnabled) {
        this.nostrColdContactEnabled = nostrColdContactEnabled;
    }
    public void setNostrUsernameDiscoveryEnabled(boolean nostrUsernameDiscoveryEnabled) {
        this.nostrUsernameDiscoveryEnabled = nostrUsernameDiscoveryEnabled;
    }
    public void setCompactEnvelopeEnabled(boolean compactEnvelopeEnabled) {
        this.compactEnvelopeEnabled = compactEnvelopeEnabled;
    }
    public void setRichPayloadEnabled(boolean richPayloadEnabled) {
        this.richPayloadEnabled = richPayloadEnabled;
    }
    public void setCryptoRecoveryEnabled(boolean cryptoRecoveryEnabled) {
        this.cryptoRecoveryEnabled = cryptoRecoveryEnabled;
    }
    public void setMeshRelay(MeshRelayConfig meshRelay) {
        this.meshRelay = meshRelay;
    }
    public void setCustody(CustodyConfig custody) {
        this.custody = custody;
    }
    public void setDataEnabled(boolean dataEnabled) {
        this.dataEnabled = dataEnabled;
    }
    public void setControlFreshnessEnforced(boolean controlFreshnessEnforced) {
        this.controlFreshnessEnforced = controlFreshnessEnforced;
    }
    public void setEncryptServiceMessages(boolean encryptServiceMessages) {
        this.encryptServiceMessages = encryptServiceMessages;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof ProtocolConfig) {
            ProtocolConfig _uniffiThat = (ProtocolConfig) _uniffiOther;
            return (
              java.util.Objects.equals(appId, _uniffiThat.appId) && 
              
              java.util.Objects.equals(profile, _uniffiThat.profile) && 
              
              bleEnabled == _uniffiThat.bleEnabled && 
              
              wifiDirectEnabled == _uniffiThat.wifiDirectEnabled && 
              
              internetEnabled == _uniffiThat.internetEnabled && 
              
              reticulumEnabled == _uniffiThat.reticulumEnabled && 
              
              nostrEnabled == _uniffiThat.nostrEnabled && 
              
              preferOnline == _uniffiThat.preferOnline && 
              
              initialTtl == _uniffiThat.initialTtl && 
              
              encryptionEnabled == _uniffiThat.encryptionEnabled && 
              
              autoKeyExchange == _uniffiThat.autoKeyExchange && 
              
              storePending == _uniffiThat.storePending && 
              
              requireEncryption == _uniffiThat.requireEncryption && 
              
              maxPendingPerPeer == _uniffiThat.maxPendingPerPeer && 
              
              maxPendingGlobal == _uniffiThat.maxPendingGlobal && 
              
              pendingTtlMs == _uniffiThat.pendingTtlMs && 
              
              java.util.Objects.equals(overflowPolicy, _uniffiThat.overflowPolicy) && 
              
              edgeDrivenUnreachableDm == _uniffiThat.edgeDrivenUnreachableDm && 
              
              maxGroupMembers == _uniffiThat.maxGroupMembers && 
              
              groupRelayEnabled == _uniffiThat.groupRelayEnabled && 
              
              groupRelayBroadcastEnabled == _uniffiThat.groupRelayBroadcastEnabled && 
              
              groupEnforceAdminCommits == _uniffiThat.groupEnforceAdminCommits && 
              
              requireTransportIdentity == _uniffiThat.requireTransportIdentity && 
              
              binaryWireEnabled == _uniffiThat.binaryWireEnabled && 
              
              nostrSealingEnabled == _uniffiThat.nostrSealingEnabled && 
              
              nostrColdContactEnabled == _uniffiThat.nostrColdContactEnabled && 
              
              nostrUsernameDiscoveryEnabled == _uniffiThat.nostrUsernameDiscoveryEnabled && 
              
              compactEnvelopeEnabled == _uniffiThat.compactEnvelopeEnabled && 
              
              richPayloadEnabled == _uniffiThat.richPayloadEnabled && 
              
              cryptoRecoveryEnabled == _uniffiThat.cryptoRecoveryEnabled && 
              
              java.util.Objects.equals(meshRelay, _uniffiThat.meshRelay) && 
              
              java.util.Objects.equals(custody, _uniffiThat.custody) && 
              
              dataEnabled == _uniffiThat.dataEnabled && 
              
              controlFreshnessEnforced == _uniffiThat.controlFreshnessEnforced && 
              
              encryptServiceMessages == _uniffiThat.encryptServiceMessages
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(appId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(profile);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(bleEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(wifiDirectEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(internetEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(reticulumEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(preferOnline);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(initialTtl);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(encryptionEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(autoKeyExchange);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(storePending);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(requireEncryption);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxPendingPerPeer);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(maxPendingGlobal);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(pendingTtlMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(overflowPolicy);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(edgeDrivenUnreachableDm);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(maxGroupMembers);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(groupRelayEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(groupRelayBroadcastEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(groupEnforceAdminCommits);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(requireTransportIdentity);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(binaryWireEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrSealingEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrColdContactEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(nostrUsernameDiscoveryEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(compactEnvelopeEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(richPayloadEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(cryptoRecoveryEnabled);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(meshRelay);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(custody);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(dataEnabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(controlFreshnessEnforced);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(encryptServiceMessages);
        return _uniffiHash;
    }

    
    
    

}


