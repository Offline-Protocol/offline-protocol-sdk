package com.offlineprotocol.sdk;

public class GatewayAddressDeclaration {
    private java.lang.String address;
    private java.util.List<java.lang.Byte> publicKey;
    private java.util.List<java.lang.Byte> signature;

    public GatewayAddressDeclaration(
        java.lang.String address, 
        java.util.List<java.lang.Byte> publicKey, 
        java.util.List<java.lang.Byte> signature
    ) {
        
        this.address = address;
        
        this.publicKey = publicKey;
        
        this.signature = signature;
    }
    
    public java.lang.String address() {
        return this.address;
    }
    
    public java.util.List<java.lang.Byte> publicKey() {
        return this.publicKey;
    }
    
    public java.util.List<java.lang.Byte> signature() {
        return this.signature;
    }
    public void setAddress(java.lang.String address) {
        this.address = address;
    }
    public void setPublicKey(java.util.List<java.lang.Byte> publicKey) {
        this.publicKey = publicKey;
    }
    public void setSignature(java.util.List<java.lang.Byte> signature) {
        this.signature = signature;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof GatewayAddressDeclaration) {
            GatewayAddressDeclaration _uniffiThat = (GatewayAddressDeclaration) _uniffiOther;
            return (
              java.util.Objects.equals(address, _uniffiThat.address) && 
              
              java.util.Objects.equals(publicKey, _uniffiThat.publicKey) && 
              
              java.util.Objects.equals(signature, _uniffiThat.signature)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(address);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(publicKey);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(signature);
        return _uniffiHash;
    }

    
    
    

}


