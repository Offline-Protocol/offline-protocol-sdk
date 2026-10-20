package com.offlineprotocol.sdk;

public class CustodyStats {
    private long held;
    private long heldBytes;
    private long accepted;
    private long delivered;
    private long reOriginated;
    private long expired;
    private long duplicates;
    private long evicted;
    private long receiptsSent;
    private long receiptsDropped;
    private long receiptsReceived;
    private long receiptsIgnored;
    private long refusedDisabled;
    private long refusedNoRequest;
    private long refusedUnknownClass;
    private long refusedNotSealed;
    private long refusedUnprovenPeer;
    private long refusedNotDepositor;
    private long refusedStranger;
    private long refusedDepositorFull;
    private long refusedStoreFull;
    private long refusedBattery;

    public CustodyStats(
        long held, 
        long heldBytes, 
        long accepted, 
        long delivered, 
        long reOriginated, 
        long expired, 
        long duplicates, 
        long evicted, 
        long receiptsSent, 
        long receiptsDropped, 
        long receiptsReceived, 
        long receiptsIgnored, 
        long refusedDisabled, 
        long refusedNoRequest, 
        long refusedUnknownClass, 
        long refusedNotSealed, 
        long refusedUnprovenPeer, 
        long refusedNotDepositor, 
        long refusedStranger, 
        long refusedDepositorFull, 
        long refusedStoreFull, 
        long refusedBattery
    ) {
        
        this.held = held;
        
        this.heldBytes = heldBytes;
        
        this.accepted = accepted;
        
        this.delivered = delivered;
        
        this.reOriginated = reOriginated;
        
        this.expired = expired;
        
        this.duplicates = duplicates;
        
        this.evicted = evicted;
        
        this.receiptsSent = receiptsSent;
        
        this.receiptsDropped = receiptsDropped;
        
        this.receiptsReceived = receiptsReceived;
        
        this.receiptsIgnored = receiptsIgnored;
        
        this.refusedDisabled = refusedDisabled;
        
        this.refusedNoRequest = refusedNoRequest;
        
        this.refusedUnknownClass = refusedUnknownClass;
        
        this.refusedNotSealed = refusedNotSealed;
        
        this.refusedUnprovenPeer = refusedUnprovenPeer;
        
        this.refusedNotDepositor = refusedNotDepositor;
        
        this.refusedStranger = refusedStranger;
        
        this.refusedDepositorFull = refusedDepositorFull;
        
        this.refusedStoreFull = refusedStoreFull;
        
        this.refusedBattery = refusedBattery;
    }
    
    public long held() {
        return this.held;
    }
    
    public long heldBytes() {
        return this.heldBytes;
    }
    
    public long accepted() {
        return this.accepted;
    }
    
    public long delivered() {
        return this.delivered;
    }
    
    public long reOriginated() {
        return this.reOriginated;
    }
    
    public long expired() {
        return this.expired;
    }
    
    public long duplicates() {
        return this.duplicates;
    }
    
    public long evicted() {
        return this.evicted;
    }
    
    public long receiptsSent() {
        return this.receiptsSent;
    }
    
    public long receiptsDropped() {
        return this.receiptsDropped;
    }
    
    public long receiptsReceived() {
        return this.receiptsReceived;
    }
    
    public long receiptsIgnored() {
        return this.receiptsIgnored;
    }
    
    public long refusedDisabled() {
        return this.refusedDisabled;
    }
    
    public long refusedNoRequest() {
        return this.refusedNoRequest;
    }
    
    public long refusedUnknownClass() {
        return this.refusedUnknownClass;
    }
    
    public long refusedNotSealed() {
        return this.refusedNotSealed;
    }
    
    public long refusedUnprovenPeer() {
        return this.refusedUnprovenPeer;
    }
    
    public long refusedNotDepositor() {
        return this.refusedNotDepositor;
    }
    
    public long refusedStranger() {
        return this.refusedStranger;
    }
    
    public long refusedDepositorFull() {
        return this.refusedDepositorFull;
    }
    
    public long refusedStoreFull() {
        return this.refusedStoreFull;
    }
    
    public long refusedBattery() {
        return this.refusedBattery;
    }
    public void setHeld(long held) {
        this.held = held;
    }
    public void setHeldBytes(long heldBytes) {
        this.heldBytes = heldBytes;
    }
    public void setAccepted(long accepted) {
        this.accepted = accepted;
    }
    public void setDelivered(long delivered) {
        this.delivered = delivered;
    }
    public void setReOriginated(long reOriginated) {
        this.reOriginated = reOriginated;
    }
    public void setExpired(long expired) {
        this.expired = expired;
    }
    public void setDuplicates(long duplicates) {
        this.duplicates = duplicates;
    }
    public void setEvicted(long evicted) {
        this.evicted = evicted;
    }
    public void setReceiptsSent(long receiptsSent) {
        this.receiptsSent = receiptsSent;
    }
    public void setReceiptsDropped(long receiptsDropped) {
        this.receiptsDropped = receiptsDropped;
    }
    public void setReceiptsReceived(long receiptsReceived) {
        this.receiptsReceived = receiptsReceived;
    }
    public void setReceiptsIgnored(long receiptsIgnored) {
        this.receiptsIgnored = receiptsIgnored;
    }
    public void setRefusedDisabled(long refusedDisabled) {
        this.refusedDisabled = refusedDisabled;
    }
    public void setRefusedNoRequest(long refusedNoRequest) {
        this.refusedNoRequest = refusedNoRequest;
    }
    public void setRefusedUnknownClass(long refusedUnknownClass) {
        this.refusedUnknownClass = refusedUnknownClass;
    }
    public void setRefusedNotSealed(long refusedNotSealed) {
        this.refusedNotSealed = refusedNotSealed;
    }
    public void setRefusedUnprovenPeer(long refusedUnprovenPeer) {
        this.refusedUnprovenPeer = refusedUnprovenPeer;
    }
    public void setRefusedNotDepositor(long refusedNotDepositor) {
        this.refusedNotDepositor = refusedNotDepositor;
    }
    public void setRefusedStranger(long refusedStranger) {
        this.refusedStranger = refusedStranger;
    }
    public void setRefusedDepositorFull(long refusedDepositorFull) {
        this.refusedDepositorFull = refusedDepositorFull;
    }
    public void setRefusedStoreFull(long refusedStoreFull) {
        this.refusedStoreFull = refusedStoreFull;
    }
    public void setRefusedBattery(long refusedBattery) {
        this.refusedBattery = refusedBattery;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof CustodyStats) {
            CustodyStats _uniffiThat = (CustodyStats) _uniffiOther;
            return (
              held == _uniffiThat.held && 
              
              heldBytes == _uniffiThat.heldBytes && 
              
              accepted == _uniffiThat.accepted && 
              
              delivered == _uniffiThat.delivered && 
              
              reOriginated == _uniffiThat.reOriginated && 
              
              expired == _uniffiThat.expired && 
              
              duplicates == _uniffiThat.duplicates && 
              
              evicted == _uniffiThat.evicted && 
              
              receiptsSent == _uniffiThat.receiptsSent && 
              
              receiptsDropped == _uniffiThat.receiptsDropped && 
              
              receiptsReceived == _uniffiThat.receiptsReceived && 
              
              receiptsIgnored == _uniffiThat.receiptsIgnored && 
              
              refusedDisabled == _uniffiThat.refusedDisabled && 
              
              refusedNoRequest == _uniffiThat.refusedNoRequest && 
              
              refusedUnknownClass == _uniffiThat.refusedUnknownClass && 
              
              refusedNotSealed == _uniffiThat.refusedNotSealed && 
              
              refusedUnprovenPeer == _uniffiThat.refusedUnprovenPeer && 
              
              refusedNotDepositor == _uniffiThat.refusedNotDepositor && 
              
              refusedStranger == _uniffiThat.refusedStranger && 
              
              refusedDepositorFull == _uniffiThat.refusedDepositorFull && 
              
              refusedStoreFull == _uniffiThat.refusedStoreFull && 
              
              refusedBattery == _uniffiThat.refusedBattery
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(held);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(heldBytes);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(accepted);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(delivered);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(reOriginated);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(expired);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(duplicates);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(evicted);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(receiptsSent);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(receiptsDropped);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(receiptsReceived);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(receiptsIgnored);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedDisabled);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedNoRequest);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedUnknownClass);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedNotSealed);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedUnprovenPeer);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedNotDepositor);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedStranger);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedDepositorFull);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedStoreFull);
        _uniffiHash = 31 * _uniffiHash + java.lang.Long.hashCode(refusedBattery);
        return _uniffiHash;
    }

    
    
    

}


