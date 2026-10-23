package com.offlineprotocol.sdk;

public class MlsAddMemberResult {
    private MlsWelcomeMessage welcome;
    private MlsEncryptedMessage commit;

    public MlsAddMemberResult(
        MlsWelcomeMessage welcome, 
        MlsEncryptedMessage commit
    ) {
        
        this.welcome = welcome;
        
        this.commit = commit;
    }
    
    public MlsWelcomeMessage welcome() {
        return this.welcome;
    }
    
    public MlsEncryptedMessage commit() {
        return this.commit;
    }
    public void setWelcome(MlsWelcomeMessage welcome) {
        this.welcome = welcome;
    }
    public void setCommit(MlsEncryptedMessage commit) {
        this.commit = commit;
    }

    

    
    @Override
    public boolean equals(java.lang.Object _uniffiOther) {
        if (_uniffiOther instanceof MlsAddMemberResult) {
            MlsAddMemberResult _uniffiThat = (MlsAddMemberResult) _uniffiOther;
            return (
              java.util.Objects.equals(welcome, _uniffiThat.welcome) && 
              
              java.util.Objects.equals(commit, _uniffiThat.commit)
              
            );
        };
        return false;
    }
    @Override
    public int hashCode() {
        int _uniffiHash = 17;
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(welcome);
        _uniffiHash = 31 * _uniffiHash + java.util.Objects.hashCode(commit);
        return _uniffiHash;
    }

    
    
    

}


