package com.offlineprotocol.sdk;

public class SendMessageOptions {
    private MessagePriority priority;
    private java.lang.String replyToMsg;
    private ContentType contentType;
    private ReplyContext replyContext;
    private MediaMetadata mediaMetadata;
    private ForwardInfo forwardInfo;
    private java.lang.String appId;

    public SendMessageOptions(
        MessagePriority priority, 
        java.lang.String replyToMsg, 
        ContentType contentType, 
        ReplyContext replyContext, 
        MediaMetadata mediaMetadata, 
        ForwardInfo forwardInfo, 
        java.lang.String appId
    ) {
        
        this.priority = priority;
        
        this.replyToMsg = replyToMsg;
        
        this.contentType = contentType;
        
        this.replyContext = replyContext;
        
        this.mediaMetadata = mediaMetadata;
        
        this.forwardInfo = forwardInfo;
        
        this.appId = appId;
    }
    
    public MessagePriority priority() {
        return this.priority;
    }
    
    public java.lang.String replyToMsg() {
        return this.replyToMsg;
    }
    
    public ContentType contentType() {
        return this.contentType;
    }
    
    public ReplyContext replyContext() {
        return this.replyContext;
    }
    
    public MediaMetadata mediaMetadata() {
        return this.mediaMetadata;
    }
    
    public ForwardInfo forwardInfo() {
        return this.forwardInfo;
    }
    
    public java.lang.String appId() {
        return this.appId;
    }
    public void setPriority(MessagePriority priority) {
        this.priority = priority;
    }
    public void setReplyToMsg(java.lang.String replyToMsg) {
        this.replyToMsg = replyToMsg;
    }
    public void setContentType(ContentType contentType) {
        this.contentType = contentType;
    }
    public void setReplyContext(ReplyContext replyContext) {
        this.replyContext = replyContext;
    }
    public void setMediaMetadata(MediaMetadata mediaMetadata) {
        this.mediaMetadata = mediaMetadata;
    }
    public void setForwardInfo(ForwardInfo forwardInfo) {
        this.forwardInfo = forwardInfo;
    }
    public void setAppId(java.lang.String appId) {
        this.appId = appId;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof SendMessageOptions) {
            SendMessageOptions _uniffiThat = (SendMessageOptions) _uniffiOther;
            return (
              java.util.Objects.equals(priority, _uniffiThat.priority) && 
              
              java.util.Objects.equals(replyToMsg, _uniffiThat.replyToMsg) && 
              
              java.util.Objects.equals(contentType, _uniffiThat.contentType) && 
              
              java.util.Objects.equals(replyContext, _uniffiThat.replyContext) && 
              
              java.util.Objects.equals(mediaMetadata, _uniffiThat.mediaMetadata) && 
              
              java.util.Objects.equals(forwardInfo, _uniffiThat.forwardInfo) && 
              
              java.util.Objects.equals(appId, _uniffiThat.appId)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(priority);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyToMsg);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(contentType);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(replyContext);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(mediaMetadata);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(forwardInfo);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(appId);
        return _uniffiHash;
    }

    
    
    

}


