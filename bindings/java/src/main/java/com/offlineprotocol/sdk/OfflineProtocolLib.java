package com.offlineprotocol.sdk;


import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
public class OfflineProtocolLib {
  public static java.lang.String deriveAddress(java.util.List<java.lang.Byte> publicKey) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_func_derive_address(_allocator, 
            FfiConverterSequenceByte.INSTANCE.lower(publicKey), _status);
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                if (ProtocolException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (ProtocolException)_uniffi_ex.getCause();
                }
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  public static InviteInfo parseInvite(java.lang.String blob) throws ProtocolException {
            try {
                return FfiConverterTypeInviteInfo.INSTANCE.lift(
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_func_parse_invite(_allocator, 
            FfiConverterString.INSTANCE.lower(blob), _status);
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                if (ProtocolException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (ProtocolException)_uniffi_ex.getCause();
                }
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  public static java.lang.String runStorageConformance(ProtocolStateStorageProvider storage)  {
            try {
                return FfiConverterString.INSTANCE.lift(
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_func_run_storage_conformance(_allocator, 
            FfiConverterTypeProtocolStateStorageProvider.INSTANCE.lower(storage), _status);
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  public static java.lang.String verifyIdentityAssertion(java.util.List<java.lang.Byte> assertion) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_func_verify_identity_assertion(_allocator, 
            FfiConverterSequenceByte.INSTANCE.lower(assertion), _status);
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                if (ProtocolException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (ProtocolException)_uniffi_ex.getCause();
                }
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

}

