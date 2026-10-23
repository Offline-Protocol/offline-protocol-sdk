package com.offlineprotocol.sdk;

public class FileProgress {
    private java.lang.String fileId;
    private int chunksSent;
    private int totalChunks;
    private byte percentage;

    public FileProgress(
        java.lang.String fileId, 
        int chunksSent, 
        int totalChunks, 
        byte percentage
    ) {
        
        this.fileId = fileId;
        
        this.chunksSent = chunksSent;
        
        this.totalChunks = totalChunks;
        
        this.percentage = percentage;
    }
    
    public java.lang.String fileId() {
        return this.fileId;
    }
    
    public int chunksSent() {
        return this.chunksSent;
    }
    
    public int totalChunks() {
        return this.totalChunks;
    }
    
    public byte percentage() {
        return this.percentage;
    }
    public void setFileId(java.lang.String fileId) {
        this.fileId = fileId;
    }
    public void setChunksSent(int chunksSent) {
        this.chunksSent = chunksSent;
    }
    public void setTotalChunks(int totalChunks) {
        this.totalChunks = totalChunks;
    }
    public void setPercentage(byte percentage) {
        this.percentage = percentage;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof FileProgress) {
            FileProgress _uniffiThat = (FileProgress) _uniffiOther;
            return (
              java.util.Objects.equals(fileId, _uniffiThat.fileId) && 
              
              chunksSent == _uniffiThat.chunksSent && 
              
              totalChunks == _uniffiThat.totalChunks && 
              
              percentage == _uniffiThat.percentage
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(fileId);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(chunksSent);
        _uniffiHash = 31 * _uniffiHash + java.lang.Integer.hashCode(totalChunks);
        _uniffiHash = 31 * _uniffiHash + java.lang.Byte.hashCode(percentage);
        return _uniffiHash;
    }

    
    
    

}


