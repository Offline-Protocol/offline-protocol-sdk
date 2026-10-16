package com.offlineprotocol.sdk;

public interface DataStoreInterface {
    public java.lang.String attachmentHash(byte[] data);
    public void counterIncrement(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, double amount) throws ProtocolException;
    public double counterValue(java.lang.String spaceId, java.lang.String docId, java.lang.String collection) throws ProtocolException;
    public void createDoc(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public void declineAttachment(java.lang.String spaceId, java.lang.String peerId, java.lang.String hash) throws ProtocolException;
    public void deleteDoc(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public java.lang.String docJson(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public long docSize(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public byte[] exportRaw(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public void fetchAttachment(java.lang.String spaceId, java.lang.String hash) throws ProtocolException;
    public void fetchAttachmentFrom(java.lang.String spaceId, java.lang.String peerId, java.lang.String hash) throws ProtocolException;
    public void flush(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public void flushAll() throws ProtocolException;
    public void listDelete(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, int index, int count) throws ProtocolException;
    public java.util.List<java.lang.String> listDocs(java.lang.String spaceId) throws ProtocolException;
    public int listLen(java.lang.String spaceId, java.lang.String docId, java.lang.String collection) throws ProtocolException;
    public void listPush(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String valueJson) throws ProtocolException;
    public java.util.List<java.lang.String> listSpaces() throws ProtocolException;
    public void mapDelete(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String key) throws ProtocolException;
    public java.lang.String mapGetJson(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String key) throws ProtocolException;
    public void mapSet(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String key, java.lang.String valueJson) throws ProtocolException;
    public void provideAttachment(java.lang.String spaceId, java.lang.String peerId, java.lang.String hash, byte[] data) throws ProtocolException;
    public void removeDoc(java.lang.String spaceId, java.lang.String docId) throws ProtocolException;
    public void removeSpace(java.lang.String spaceId) throws ProtocolException;
    public void setInterest(java.lang.String spaceId, java.util.List<java.lang.String> patterns) throws ProtocolException;
    public void textDelete(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, int position, int count) throws ProtocolException;
    public void textInsert(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, int position, java.lang.String text) throws ProtocolException;
    public java.lang.String textValue(java.lang.String spaceId, java.lang.String docId, java.lang.String collection) throws ProtocolException;
    public void wipeAll() throws ProtocolException;
    
}

