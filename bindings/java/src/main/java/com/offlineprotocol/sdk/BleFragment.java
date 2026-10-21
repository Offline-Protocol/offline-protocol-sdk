package com.offlineprotocol.sdk;

public class BleFragment {
    private java.lang.String recipientId;
    private java.util.List<java.lang.Byte> data;

    public BleFragment(
        java.lang.String recipientId, 
        java.util.List<java.lang.Byte> data
    ) {
        
        this.recipientId = recipientId;
        
        this.data = data;
    }
    
    public java.lang.String recipientId() {
        return this.recipientId;
    }
    
    public java.util.List<java.lang.Byte> data() {
        return this.data;
    }
    public void setRecipientId(java.lang.String recipientId) {
        this.recipientId = recipientId;
    }
    public void setData(java.util.List<java.lang.Byte> data) {
        this.data = data;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof BleFragment) {
            BleFragment _uniffiThat = (BleFragment) _uniffiOther;
            return (
              java.util.Objects.equals(recipientId, _uniffiThat.recipientId) && 
              
              java.util.Objects.equals(data, _uniffiThat.data)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(recipientId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(data);
        return _uniffiHash;
    }

    
    
    

}


