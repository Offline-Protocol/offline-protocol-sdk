package com.offlineprotocol.sdk;

public class MediaMetadata {
    private java.lang.String mimeType;
    private java.lang.String fileName;
    private long fileSize;
    private java.lang.Long durationMs;
    private java.lang.Integer width;
    private java.lang.Integer height;
    private java.lang.String thumbnailBase64;
    private java.lang.String mediaId;
    private java.lang.String downloadUrl;
    private java.lang.String thumbnailUrl;
    private java.lang.String encryptionKey;
    private java.lang.String iv;
    private java.lang.String ciphertextHash;
    private java.lang.String stickerProvider;
    private java.lang.String stickerRemoteId;
    private java.lang.String stickerKind;

    public MediaMetadata(
        java.lang.String mimeType, 
        java.lang.String fileName, 
        long fileSize, 
        java.lang.Long durationMs, 
        java.lang.Integer width, 
        java.lang.Integer height, 
        java.lang.String thumbnailBase64, 
        java.lang.String mediaId, 
        java.lang.String downloadUrl, 
        java.lang.String thumbnailUrl, 
        java.lang.String encryptionKey, 
        java.lang.String iv, 
        java.lang.String ciphertextHash, 
        java.lang.String stickerProvider, 
        java.lang.String stickerRemoteId, 
        java.lang.String stickerKind
    ) {
        
        this.mimeType = mimeType;
        
        this.fileName = fileName;
        
        this.fileSize = fileSize;
        
        this.durationMs = durationMs;
        
        this.width = width;
        
        this.height = height;
        
        this.thumbnailBase64 = thumbnailBase64;
        
        this.mediaId = mediaId;
        
        this.downloadUrl = downloadUrl;
        
        this.thumbnailUrl = thumbnailUrl;
        
        this.encryptionKey = encryptionKey;
        
        this.iv = iv;
        
        this.ciphertextHash = ciphertextHash;
        
        this.stickerProvider = stickerProvider;
        
        this.stickerRemoteId = stickerRemoteId;
        
        this.stickerKind = stickerKind;
    }
    
    public java.lang.String mimeType() {
        return this.mimeType;
    }
    
    public java.lang.String fileName() {
        return this.fileName;
    }
    
    public long fileSize() {
        return this.fileSize;
    }
    
    public java.lang.Long durationMs() {
        return this.durationMs;
    }
    
    public java.lang.Integer width() {
        return this.width;
    }
    
    public java.lang.Integer height() {
        return this.height;
    }
    
    public java.lang.String thumbnailBase64() {
        return this.thumbnailBase64;
    }
    
    public java.lang.String mediaId() {
        return this.mediaId;
    }
    
    public java.lang.String downloadUrl() {
        return this.downloadUrl;
    }
    
    public java.lang.String thumbnailUrl() {
        return this.thumbnailUrl;
    }
    
    public java.lang.String encryptionKey() {
        return this.encryptionKey;
    }
    
    public java.lang.String iv() {
        return this.iv;
    }
    
    public java.lang.String ciphertextHash() {
        return this.ciphertextHash;
    }
    
    public java.lang.String stickerProvider() {
        return this.stickerProvider;
    }
    
    public java.lang.String stickerRemoteId() {
        return this.stickerRemoteId;
    }
    
    public java.lang.String stickerKind() {
        return this.stickerKind;
    }
    public void setMimeType(java.lang.String mimeType) {
        this.mimeType = mimeType;
    }
    public void setFileName(java.lang.String fileName) {
        this.fileName = fileName;
    }
    public void setFileSize(long fileSize) {
        this.fileSize = fileSize;
    }
    public void setDurationMs(java.lang.Long durationMs) {
        this.durationMs = durationMs;
    }
    public void setWidth(java.lang.Integer width) {
        this.width = width;
    }
    public void setHeight(java.lang.Integer height) {
        this.height = height;
    }
    public void setThumbnailBase64(java.lang.String thumbnailBase64) {
        this.thumbnailBase64 = thumbnailBase64;
    }
    public void setMediaId(java.lang.String mediaId) {
        this.mediaId = mediaId;
    }
    public void setDownloadUrl(java.lang.String downloadUrl) {
        this.downloadUrl = downloadUrl;
    }
    public void setThumbnailUrl(java.lang.String thumbnailUrl) {
        this.thumbnailUrl = thumbnailUrl;
    }
    public void setEncryptionKey(java.lang.String encryptionKey) {
        this.encryptionKey = encryptionKey;
    }
    public void setIv(java.lang.String iv) {
        this.iv = iv;
    }
    public void setCiphertextHash(java.lang.String ciphertextHash) {
        this.ciphertextHash = ciphertextHash;
    }
    public void setStickerProvider(java.lang.String stickerProvider) {
        this.stickerProvider = stickerProvider;
    }
    public void setStickerRemoteId(java.lang.String stickerRemoteId) {
        this.stickerRemoteId = stickerRemoteId;
    }
    public void setStickerKind(java.lang.String stickerKind) {
        this.stickerKind = stickerKind;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MediaMetadata) {
            MediaMetadata _uniffiThat = (MediaMetadata) _uniffiOther;
            return (
              java.util.Objects.equals(mimeType, _uniffiThat.mimeType) && 
              
              java.util.Objects.equals(fileName, _uniffiThat.fileName) && 
              
              fileSize == _uniffiThat.fileSize && 
              
              java.util.Objects.equals(durationMs, _uniffiThat.durationMs) && 
              
              java.util.Objects.equals(width, _uniffiThat.width) && 
              
              java.util.Objects.equals(height, _uniffiThat.height) && 
              
              java.util.Objects.equals(thumbnailBase64, _uniffiThat.thumbnailBase64) && 
              
              java.util.Objects.equals(mediaId, _uniffiThat.mediaId) && 
              
              java.util.Objects.equals(downloadUrl, _uniffiThat.downloadUrl) && 
              
              java.util.Objects.equals(thumbnailUrl, _uniffiThat.thumbnailUrl) && 
              
              java.util.Objects.equals(encryptionKey, _uniffiThat.encryptionKey) && 
              
              java.util.Objects.equals(iv, _uniffiThat.iv) && 
              
              java.util.Objects.equals(ciphertextHash, _uniffiThat.ciphertextHash) && 
              
              java.util.Objects.equals(stickerProvider, _uniffiThat.stickerProvider) && 
              
              java.util.Objects.equals(stickerRemoteId, _uniffiThat.stickerRemoteId) && 
              
              java.util.Objects.equals(stickerKind, _uniffiThat.stickerKind)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mimeType);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(fileName);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(fileSize);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(durationMs);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(width);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(height);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(thumbnailBase64);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mediaId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(downloadUrl);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(thumbnailUrl);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(encryptionKey);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(iv);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(ciphertextHash);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(stickerProvider);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(stickerRemoteId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(stickerKind);
        return _uniffiHash;
    }

    
    
    

}


