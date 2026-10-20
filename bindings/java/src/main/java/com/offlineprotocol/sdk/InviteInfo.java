package com.offlineprotocol.sdk;

public class InviteInfo {
    private java.lang.String address;
    private java.util.List<java.lang.Byte> publicKey;
    private java.lang.String petname;
    private boolean signed;

    public InviteInfo(
        java.lang.String address, 
        java.util.List<java.lang.Byte> publicKey, 
        java.lang.String petname, 
        boolean signed
    ) {
        
        this.address = address;
        
        this.publicKey = publicKey;
        
        this.petname = petname;
        
        this.signed = signed;
    }
    
    public java.lang.String address() {
        return this.address;
    }
    
    public java.util.List<java.lang.Byte> publicKey() {
        return this.publicKey;
    }
    
    public java.lang.String petname() {
        return this.petname;
    }
    
    public boolean signed() {
        return this.signed;
    }
    public void setAddress(java.lang.String address) {
        this.address = address;
    }
    public void setPublicKey(java.util.List<java.lang.Byte> publicKey) {
        this.publicKey = publicKey;
    }
    public void setPetname(java.lang.String petname) {
        this.petname = petname;
    }
    public void setSigned(boolean signed) {
        this.signed = signed;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof InviteInfo) {
            InviteInfo _uniffiThat = (InviteInfo) _uniffiOther;
            return (
              java.util.Objects.equals(address, _uniffiThat.address) && 
              
              java.util.Objects.equals(publicKey, _uniffiThat.publicKey) && 
              
              java.util.Objects.equals(petname, _uniffiThat.petname) && 
              
              signed == _uniffiThat.signed
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(address);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(publicKey);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(petname);
        _uniffiHash = 31 * _uniffiHash + java.lang.Boolean.hashCode(signed);
        return _uniffiHash;
    }

    
    
    

}


