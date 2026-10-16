package com.offlineprotocol.sdk;

public class MediaSendOptions {
    private MediaMetadata mediaMetadata;
    private java.lang.String caption;
    private java.lang.String replyToMsg;
    private ReplyContext replyContext;
    private ForwardInfo forwardInfo;
    private java.lang.String fileId;
    private java.lang.String appId;

    public MediaSendOptions(
        MediaMetadata mediaMetadata, 
        java.lang.String caption, 
        java.lang.String replyToMsg, 
        ReplyContext replyContext, 
        ForwardInfo forwardInfo, 
        java.lang.String fileId, 
        java.lang.String appId
    ) {
        
        this.mediaMetadata = mediaMetadata;
        
        this.caption = caption;
        
        this.replyToMsg = replyToMsg;
        
        this.replyContext = replyContext;
        
        this.forwardInfo = forwardInfo;
        
        this.fileId = fileId;
        
        this.appId = appId;
    }
    
    public MediaMetadata mediaMetadata() {
        return this.mediaMetadata;
    }
    
    public java.lang.String caption() {
        return this.caption;
    }
    
    public java.lang.String replyToMsg() {
        return this.replyToMsg;
    }
    
    public ReplyContext replyContext() {
        return this.replyContext;
    }
    
    public ForwardInfo forwardInfo() {
        return this.forwardInfo;
    }
    
    public java.lang.String fileId() {
        return this.fileId;
    }
    
    public java.lang.String appId() {
        return this.appId;
    }
    public void setMediaMetadata(MediaMetadata mediaMetadata) {
        this.mediaMetadata = mediaMetadata;
    }
    public void setCaption(java.lang.String caption) {
        this.caption = caption;
    }
    public void setReplyToMsg(java.lang.String replyToMsg) {
        this.replyToMsg = replyToMsg;
    }
    public void setReplyContext(ReplyContext replyContext) {
        this.replyContext = replyContext;
    }
    public void setForwardInfo(ForwardInfo forwardInfo) {
        this.forwardInfo = forwardInfo;
    }
    public void setFileId(java.lang.String fileId) {
        this.fileId = fileId;
    }
    public void setAppId(java.lang.String appId) {
        this.appId = appId;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MediaSendOptions) {
            MediaSendOptions _uniffiThat = (MediaSendOptions) _uniffiOther;
            return (
              java.util.Objects.equals(mediaMetadata, _uniffiThat.mediaMetadata) && 
              
              java.util.Objects.equals(caption, _uniffiThat.caption) && 
              
              java.util.Objects.equals(replyToMsg, _uniffiThat.replyToMsg) && 
              
              java.util.Objects.equals(replyContext, _uniffiThat.replyContext) && 
              
              java.util.Objects.equals(forwardInfo, _uniffiThat.forwardInfo) && 
              
              java.util.Objects.equals(fileId, _uniffiThat.fileId) && 
              
              java.util.Objects.equals(appId, _uniffiThat.appId)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mediaMetadata);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(caption);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyToMsg);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyContext);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(forwardInfo);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(fileId);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(appId);
        return _uniffiHash;
    }

    
    
    

}


