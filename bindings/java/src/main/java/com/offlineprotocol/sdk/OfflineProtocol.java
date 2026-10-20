package com.offlineprotocol.sdk;

public class OfflineProtocol implements AutoCloseable, OfflineProtocolInterface {
  protected long handle;
  protected UniffiCleaner.Cleanable cleanable;

  private java.util.concurrent.atomic.AtomicBoolean wasDestroyed = new java.util.concurrent.atomic.AtomicBoolean(false);
  private java.util.concurrent.atomic.AtomicLong callCounter = new java.util.concurrent.atomic.AtomicLong(1);

  /**
   * Internal constructor to wrap a raw handle from FFI.
   * The UniffiWithHandle marker disambiguates this from other constructors.
   */
  public OfflineProtocol(UniffiWithHandle phantom, long handle) {
    this.handle = handle;
    this.cleanable = UniffiLib.CLEANER.register(this, new UniffiCleanAction(handle));
  }

  /**
   * This constructor can be used to instantiate a fake object. Only used for tests. Any
   * attempt to actually use an object constructed this way will fail as there is no
   * connected Rust object.
   */
  public OfflineProtocol(NoHandle noHandle) {
    this.handle = 0L;
    this.cleanable = null;
  }

  
  public OfflineProtocol(ProtocolConfig config) throws ProtocolException{
    this(UniffiWithHandle.INSTANCE, (long)
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_constructor_offlineprotocol_new(
            FfiConverterTypeProtocolConfig.INSTANCE.lower(config), _status);
    })
    );
  }

  @Override
  public synchronized void close() {
    // Only allow a single call to this method.
    // TODO(uniffi): maybe we should log a warning if called more than once?
    if (this.wasDestroyed.compareAndSet(false, true)) {
      // This decrement always matches the initial count of 1 given at creation time.
      if (this.callCounter.decrementAndGet() == 0L) {
        if (cleanable != null) {
          cleanable.clean();
        }
      }
    }
  }

  public <R> R callWithHandle(java.util.function.Function<java.lang.Long, R> block) {
    // Check and increment the call counter, to keep the object alive.
    // This needs a compare-and-set retry loop in case of concurrent updates.
    long c;
    do {
      c = this.callCounter.get();
      if (c == 0L) {
        throw new java.lang.IllegalStateException("OfflineProtocol object has already been destroyed");
      }
      if (c == java.lang.Long.MAX_VALUE) {
        throw new java.lang.IllegalStateException("OfflineProtocol call counter would overflow");
      }
    } while (! this.callCounter.compareAndSet(c, c + 1L));
    // Now we can safely do the method call without the handle being freed concurrently.
    try {
      return block.apply(this.uniffiCloneHandle());
    } finally {
      // This decrement always matches the increment we performed above.
      if (this.callCounter.decrementAndGet() == 0L) {
        if (cleanable != null) {
          cleanable.clean();
        }
      }
    }
  }

  public void callWithHandle(java.util.function.Consumer<java.lang.Long> block) {
    callWithHandle((java.lang.Long uniffiHandle) -> {
      block.accept(uniffiHandle);
      return (java.lang.Void)null;
    });
  }

  private class UniffiCleanAction implements Runnable {
    private final long handle;

    public UniffiCleanAction(long handle) {
      this.handle = handle;
    }

    @Override
    public void run() {
      // If the handle is 0 this is a fake object created with `NoHandle`, don't try to free.
      if (handle != 0L) {
        UniffiHelpers.uniffiRustCall((_allocator, status) -> {
          UniffiLib.uniffi_offline_protocol_uniffi_fn_free_offlineprotocol(handle, status);
          return null;
        });
      }
    }
  }

  long uniffiCloneHandle() {
    return UniffiHelpers.uniffiRustCall((_allocator, status) -> {
      if (handle == 0L) {
        throw new java.lang.NullPointerException();
      }
      return UniffiLib.uniffi_offline_protocol_uniffi_fn_clone_offlineprotocol(handle, status);
    });
  }

  
    @Override
    public java.lang.String acceptConnectionRequest(java.lang.String recipient, java.lang.String accepterName, java.util.List<java.lang.Byte> keyPackage) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_accept_connection_request(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterString.INSTANCE.lower(accepterName), FfiConverterOptionalSequenceByte.INSTANCE.lower(keyPackage), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void bleClearPeerMtu(java.lang.String peerId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_clear_peer_mtu(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public long bleFragmentFallbackCount()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallLong( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_fallback_count(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void bleFragmentReceived(java.lang.String senderId, java.util.List<java.lang.Byte> fragment) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_fragment_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(senderId), FfiConverterSequenceByte.INSTANCE.lower(fragment), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public BleFragment bleGetNextFragment()  {
            try {
                return FfiConverterOptionalTypeBleFragment.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_next_fragment(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public int bleGetPeerCount()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallInt( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_get_peer_count(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void blePeerDiscovered(java.lang.String peerId, short rssi) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_discovered(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), rssi, _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void blePeerLost(java.lang.String peerId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_peer_lost(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public long bleRecipientNotAmongPeersCount()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallLong( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_recipient_not_among_peers_count(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void bleReturnFragment()  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_return_fragment(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void bleSetPeerMtu(java.lang.String peerId, int maxPayload) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_set_peer_mtu(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), maxPayload, _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void bleStatusChanged(boolean isAvailable) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_status_changed(_uniffiHandle,
            FfiConverterBoolean.INSTANCE.lower(isAvailable), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public long bleUndersizedMtuReports()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallLong( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_ble_undersized_mtu_reports(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void blockUser(java.lang.String userId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_block_user(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(userId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public java.lang.String cancelConnectionRequest(java.lang.String recipient) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_connection_request(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void cancelFileTransfer(java.lang.String fileId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_cancel_file_transfer(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(fileId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void closeFileStores() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_close_file_stores(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public MlsGroupInfo createGroup(java.lang.String groupName) throws ProtocolException {
            try {
                return FfiConverterTypeMlsGroupInfo.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_group(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupName), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String createInvite(java.lang.String petname, boolean sign) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_create_invite(_allocator, _uniffiHandle,
            FfiConverterOptionalString.INSTANCE.lower(petname), FfiConverterBoolean.INSTANCE.lower(sign), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String deriveUserIdFromPublicKey(java.util.List<java.lang.Byte> publicKey)  {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_derive_user_id_from_public_key(_allocator, _uniffiHandle,
            FfiConverterSequenceByte.INSTANCE.lower(publicKey), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void disableTelemetry() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_disable_telemetry(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void emitTestEvent()  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_emit_test_event(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void enableTelemetry(TelemetryConfig config, AppState appState) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_enable_telemetry(_uniffiHandle,
            FfiConverterTypeTelemetryConfig.INSTANCE.lower(config), FfiConverterTypeAppState.INSTANCE.lower(appState), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void endTelemetrySession()  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_end_telemetry_session(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void eraseCustody() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_erase_custody(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public MlsWelcomeMessage establishSecureSession(java.lang.String peerId) throws ProtocolException {
            try {
                return FfiConverterOptionalTypeMlsWelcomeMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_establish_secure_session(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void finalizeFile(java.lang.String fileId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_finalize_file(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(fileId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void flushTelemetry()  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public boolean flushTelemetryBlocking(long deadlineMs)  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_flush_telemetry_blocking(_uniffiHandle,
            deadlineMs, _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void forceTransport(TransportType transportType) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_force_transport(_uniffiHandle,
            FfiConverterTypeTransportType.INSTANCE.lower(transportType), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public java.lang.String forwardMessage(java.lang.String originalMessageJson, java.lang.String newRecipient, MessagePriority priority) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(originalMessageJson), FfiConverterString.INSTANCE.lower(newRecipient), FfiConverterOptionalTypeMessagePriority.INSTANCE.lower(priority), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.String> forwardMessageToGroup(java.lang.String originalMessageJson, java.lang.String groupId, MessagePriority priority) throws ProtocolException {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_forward_message_to_group(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(originalMessageJson), FfiConverterString.INSTANCE.lower(groupId), FfiConverterOptionalTypeMessagePriority.INSTANCE.lower(priority), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public GatewayAddressDeclaration gatewayAddressDeclaration(java.util.List<java.lang.Byte> challenge) throws ProtocolException {
            try {
                return FfiConverterTypeGatewayAddressDeclaration.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_gateway_address_declaration(_allocator, _uniffiHandle,
            FfiConverterSequenceByte.INSTANCE.lower(challenge), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.String> getActiveTransports()  {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_active_transports(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.lang.Byte getBatteryLevel()  {
            try {
                return FfiConverterOptionalByte.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_battery_level(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.util.List<java.lang.String> getBlockedUsers() throws ProtocolException {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_blocked_users(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public CustodyStats getCustodyStats()  {
            try {
                return FfiConverterTypeCustodyStats.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_custody_stats(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public DedupStats getDedupStats()  {
            try {
                return FfiConverterTypeDedupStats.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dedup_stats(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public float getDeliverySuccessRate()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallFloat( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_delivery_success_rate(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public DorsConfig getDorsConfig()  {
            try {
                return FfiConverterTypeDorsConfig.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_dors_config(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public EstablishmentState getEstablishmentState(java.lang.String peerId) throws ProtocolException {
            try {
                return FfiConverterTypeEstablishmentState.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_establishment_state(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public FileProgress getFileProgress(java.lang.String fileId)  {
            try {
                return FfiConverterOptionalTypeFileProgress.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_file_progress(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(fileId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public MlsGroupInfo getGroupInfo(java.lang.String groupId) throws ProtocolException {
            try {
                return FfiConverterOptionalTypeMlsGroupInfo.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_info(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.Map<java.lang.String, java.lang.String> getGroupRoles(java.lang.String groupId) throws ProtocolException {
            try {
                return FfiConverterMapStringString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_group_roles(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.Byte> getIdentityPublicKey() throws ProtocolException {
            try {
                return FfiConverterSequenceByte.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_identity_public_key(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public boolean getIsCharging()  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_is_charging(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public int getMediaQueueDepth(java.lang.String recipient) throws ProtocolException {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithErrorInt(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_media_queue_depth(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public byte getMedianHops()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallByte( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_hops(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public long getMedianLatency()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallLong( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_median_latency(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.lang.String getMemberRole(java.lang.String groupId, java.lang.String userId) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_member_role(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), FfiConverterString.INSTANCE.lower(userId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public MeshRelayStats getMeshRelayStats()  {
            try {
                return FfiConverterTypeMeshRelayStats.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_stats(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public MeshRelayTunables getMeshRelayTunables()  {
            try {
                return FfiConverterTypeMeshRelayTunables.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_mesh_relay_tunables(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.util.List<MessageStats> getMessageStats()  {
            try {
                return FfiConverterSequenceTypeMessageStats.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_message_stats(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public long getPendingAckCount()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallLong( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_pending_ack_count(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public RelayConfig getRelayConfig()  {
            try {
                return FfiConverterTypeRelayConfig.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_config(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public RelayPriority getRelayPriority()  {
            try {
                return FfiConverterTypeRelayPriority.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_relay_priority(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public long getRetryQueueSize()  {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallLong( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_retry_queue_size(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public ProtocolState getState()  {
            try {
                return FfiConverterTypeProtocolState.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_state(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public NetworkTopology getTopology() throws ProtocolException {
            try {
                return FfiConverterTypeNetworkTopology.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_topology(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public TransportMetrics getTransportMetrics(TransportType transportType)  {
            try {
                return FfiConverterOptionalTypeTransportMetrics.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_get_transport_metrics(_allocator, _uniffiHandle,
            FfiConverterTypeTransportType.INSTANCE.lower(transportType), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public RelaySyncState groupRelaySyncState(java.lang.String groupId) throws ProtocolException {
            try {
                return FfiConverterTypeRelaySyncState.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_relay_sync_state(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public GroupRichReadiness groupRichReadiness(java.lang.String groupId) throws ProtocolException {
            try {
                return FfiConverterTypeGroupRichReadiness.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_group_rich_readiness(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public boolean hasPendingKeyPackage(java.lang.String peerId)  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_has_pending_key_package(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.util.List<java.lang.Byte> identityAssertion(java.util.List<java.lang.Byte> signedData) throws ProtocolException {
            try {
                return FfiConverterSequenceByte.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_identity_assertion(_allocator, _uniffiHandle,
            FfiConverterSequenceByte.INSTANCE.lower(signedData), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void initializeMls(MlsStorageProvider secureStorage, ProtocolStateStorageProvider protocolStateStorage) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls(_uniffiHandle,
            FfiConverterTypeMlsStorageProvider.INSTANCE.lower(secureStorage), FfiConverterTypeProtocolStateStorageProvider.INSTANCE.lower(protocolStateStorage), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void initializeMlsWithFileStores(java.lang.String mlsRoot, java.lang.String stateRoot, byte[] storeKey) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_initialize_mls_with_file_stores(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(mlsRoot), FfiConverterString.INSTANCE.lower(stateRoot), FfiConverterByteArray.INSTANCE.lower(storeKey), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void internetAddressDeclarationRefused(java.lang.String reason)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declaration_refused(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(reason), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void internetAddressDeclared(java.lang.String address)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_address_declared(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(address), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void internetConfirmSent(java.lang.String messageId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_confirm_sent(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public InternetMessage internetGetNextMessage()  {
            try {
                return FfiConverterOptionalTypeInternetMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_get_next_message(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void internetGroupReportReceived(java.lang.String reportJson) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_group_report_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(reportJson), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void internetMessageReceived(java.lang.String senderId, java.util.List<java.lang.Byte> data) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_message_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(senderId), FfiConverterSequenceByte.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void internetPeerPresence(java.lang.String peerId, boolean online, java.lang.Long lastSeenMs)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_peer_presence(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), FfiConverterBoolean.INSTANCE.lower(online), FfiConverterOptionalLong.INSTANCE.lower(lastSeenMs), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.util.List<java.lang.String> internetPresenceWatchlist()  {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_presence_watchlist(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void internetRelayCapabilities(java.util.List<java.lang.String> capabilities) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_relay_capabilities(_uniffiHandle,
            FfiConverterSequenceString.INSTANCE.lower(capabilities), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void internetSendFailed(java.lang.String messageId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void internetSendFailedWithReason(java.lang.String messageId, java.lang.String reason)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_send_failed_with_reason(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), FfiConverterOptionalString.INSTANCE.lower(reason), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void internetStatusChanged(boolean isConnected) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_internet_status_changed(_uniffiHandle,
            FfiConverterBoolean.INSTANCE.lower(isConnected), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void inviteToGroup(java.lang.String groupId, java.lang.String inviteeUserId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_invite_to_group(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), FfiConverterString.INSTANCE.lower(inviteeUserId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public boolean isMlsInitialized()  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_mls_initialized(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public boolean isRelay()  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_relay(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public boolean isUserBlocked(java.lang.String userId) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_is_user_blocked(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(userId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void leaveGroup(java.lang.String groupId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_leave_group(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public java.util.List<java.lang.String> listGroups() throws ProtocolException {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_list_groups(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String localAddress()  {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_local_address(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void mlsClearPendingWelcome(java.lang.String otherUserId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_clear_pending_welcome(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(otherUserId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public MlsWelcomeMessage mlsCreateSession(java.lang.String otherUserId) throws ProtocolException {
            try {
                return FfiConverterTypeMlsWelcomeMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_create_session(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(otherUserId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.Byte> mlsDecrypt(MlsEncryptedMessage encrypted) throws ProtocolException {
            try {
                return FfiConverterOptionalSequenceByte.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt(_allocator, _uniffiHandle,
            FfiConverterTypeMlsEncryptedMessage.INSTANCE.lower(encrypted), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.Byte> mlsDecryptFromUser(MlsEncryptedMessage encrypted) throws ProtocolException {
            try {
                return FfiConverterOptionalSequenceByte.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_decrypt_from_user(_allocator, _uniffiHandle,
            FfiConverterTypeMlsEncryptedMessage.INSTANCE.lower(encrypted), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void mlsDeleteSession(java.lang.String otherUserId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_delete_session(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(otherUserId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public MlsEncryptedMessage mlsEncryptForUser(java.lang.String otherUserId, java.util.List<java.lang.Byte> plaintext) throws ProtocolException {
            try {
                return FfiConverterTypeMlsEncryptedMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_encrypt_for_user(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(otherUserId), FfiConverterSequenceByte.INSTANCE.lower(plaintext), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public MlsKeyPackageBundle mlsGenerateKeyPackage() throws ProtocolException {
            try {
                return FfiConverterTypeMlsKeyPackageBundle.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_generate_key_package(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public MlsKeyPackageBundle mlsGetOrCreateKeyPackage() throws ProtocolException {
            try {
                return FfiConverterTypeMlsKeyPackageBundle.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_or_create_key_package(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<MlsKeyPackageBundle> mlsGetPendingKeyPackages()  {
            try {
                return FfiConverterSequenceTypeMlsKeyPackageBundle.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_key_packages(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public MlsWelcomeMessage mlsGetPendingWelcome(java.lang.String otherUserId)  {
            try {
                return FfiConverterOptionalTypeMlsWelcomeMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_get_pending_welcome(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(otherUserId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public boolean mlsHasSession(java.lang.String otherUserId)  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_has_session(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(otherUserId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void mlsImportKeyPackage(java.lang.String userId, java.util.List<java.lang.Byte> keyPackageData) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_import_key_package(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(userId), FfiConverterSequenceByte.INSTANCE.lower(keyPackageData), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public MlsGroupInfo mlsJoinSession(MlsWelcomeMessage welcome) throws ProtocolException {
            try {
                return FfiConverterTypeMlsGroupInfo.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_join_session(_allocator, _uniffiHandle,
            FfiConverterTypeMlsWelcomeMessage.INSTANCE.lower(welcome), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.String> mlsListSessions()  {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_list_sessions(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void mlsMarkKeyPackageSynced(java.lang.String packageId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_mark_key_package_synced(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(packageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public MlsGroupInfo mlsProcessWelcome(MlsWelcomeMessage welcome) throws ProtocolException {
            try {
                return FfiConverterTypeMlsGroupInfo.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_mls_process_welcome(_allocator, _uniffiHandle,
            FfiConverterTypeMlsWelcomeMessage.INSTANCE.lower(welcome), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void nostrConfirmSent(java.lang.String messageId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_confirm_sent(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public NostrMessage nostrGetNextMessage()  {
            try {
                return FfiConverterOptionalTypeNostrMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_message(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public NostrQuery nostrGetNextQuery()  {
            try {
                return FfiConverterOptionalTypeNostrQuery.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_next_query(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.lang.String nostrGetPublicKey()  {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_public_key(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.lang.String nostrGetSubscriptionFilter(java.lang.String subscriptionId)  {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_get_subscription_filter(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(subscriptionId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void nostrMessageReceived(java.lang.String senderId, java.util.List<java.lang.Byte> data) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(senderId), FfiConverterSequenceByte.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void nostrMessageReceivedAt(java.lang.String senderId, java.util.List<java.lang.Byte> data, long createdAt) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_message_received_at(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(senderId), FfiConverterSequenceByte.INSTANCE.lower(data), createdAt, _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void nostrQueryCompleted(java.lang.String queryId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_completed(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(queryId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void nostrQueryEventReceived(java.lang.String queryId, java.lang.String eventJson) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_query_event_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(queryId), FfiConverterString.INSTANCE.lower(eventJson), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void nostrSendFailed(java.lang.String messageId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void nostrSendFailedWithReason(java.lang.String messageId, java.lang.String reason)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_send_failed_with_reason(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), FfiConverterOptionalString.INSTANCE.lower(reason), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void nostrStatusChanged(boolean isConnected) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_nostr_status_changed(_uniffiHandle,
            FfiConverterBoolean.INSTANCE.lower(isConnected), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void notifyAppState(AppState state)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_notify_app_state(_uniffiHandle,
            FfiConverterTypeAppState.INSTANCE.lower(state), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void pause() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_pause(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public java.lang.String pollEvent()  {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_poll_event(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void process() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void processFileChunk(java.lang.String fileId, int chunkIndex, int totalChunks, long fileSize, java.lang.String fileName, java.lang.String fileChecksum, java.util.List<java.lang.Byte> data) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_process_file_chunk(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(fileId), chunkIndex, totalChunks, fileSize, FfiConverterString.INSTANCE.lower(fileName), FfiConverterString.INSTANCE.lower(fileChecksum), FfiConverterSequenceByte.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public ProtocolLockDiagnostics protocolLockDiagnostics()  {
            try {
                return FfiConverterTypeProtocolLockDiagnostics.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_protocol_lock_diagnostics(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.lang.String receiveMessage()  {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_receive_message(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.lang.String rejectConnectionRequest(java.lang.String recipient) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reject_connection_request(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public boolean rekeySession(java.lang.String peerId) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rekey_session(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void releaseTransportLock()  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_release_transport_lock(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void removeFromGroup(java.lang.String groupId, java.lang.String memberId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_from_group(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), FfiConverterString.INSTANCE.lower(memberId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void removeTransport(TransportType transportType) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_remove_transport(_uniffiHandle,
            FfiConverterTypeTransportType.INSTANCE.lower(transportType), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void renameGroup(java.lang.String groupId, java.lang.String newName) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_rename_group(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), FfiConverterString.INSTANCE.lower(newName), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public boolean requestGroupRelayRegistration(java.lang.String groupId) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_request_group_relay_registration(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public boolean resolveUsername(java.lang.String username) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resolve_username(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(username), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void resume() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_resume(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void reticulumAddressDeclarationRefused(java.lang.String reason)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declaration_refused(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(reason), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumAddressDeclared(java.lang.String address)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_address_declared(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(address), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumConfirmSent(java.lang.String messageId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_confirm_sent(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumGatewayCapabilities(java.util.List<java.lang.String> capabilities) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_gateway_capabilities(_uniffiHandle,
            FfiConverterSequenceString.INSTANCE.lower(capabilities), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public ReticulumMessage reticulumGetNextMessage()  {
            try {
                return FfiConverterOptionalTypeReticulumMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_get_next_message(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumMessageReceived(java.lang.String senderId, java.util.List<java.lang.Byte> data) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_message_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(senderId), FfiConverterSequenceByte.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void reticulumPeerPresence(java.lang.String peerId, boolean online, java.lang.Long lastSeenMs)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_peer_presence(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), FfiConverterBoolean.INSTANCE.lower(online), FfiConverterOptionalLong.INSTANCE.lower(lastSeenMs), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.util.List<java.lang.String> reticulumPresenceWatchlist()  {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_presence_watchlist(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumSendFailed(java.lang.String messageId)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumSendFailedWithReason(java.lang.String messageId, java.lang.String reason)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_send_failed_with_reason(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(messageId), FfiConverterOptionalString.INSTANCE.lower(reason), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void reticulumStatusChanged(boolean isConnected) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_reticulum_status_changed(_uniffiHandle,
            FfiConverterBoolean.INSTANCE.lower(isConnected), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public java.lang.String sendConnectionRequest(java.lang.String recipient, java.lang.String senderName, java.util.List<java.lang.Byte> keyPackage, java.lang.String initialMessage) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_connection_request(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterString.INSTANCE.lower(senderName), FfiConverterOptionalSequenceByte.INSTANCE.lower(keyPackage), FfiConverterOptionalString.INSTANCE.lower(initialMessage), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendFile(java.lang.String recipient, java.util.List<java.lang.Byte> fileData, java.lang.String fileName) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterSequenceByte.INSTANCE.lower(fileData), FfiConverterString.INSTANCE.lower(fileName), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendFileFromPath(java.lang.String recipient, java.lang.String filePath, java.lang.String fileName) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_file_from_path(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterString.INSTANCE.lower(filePath), FfiConverterString.INSTANCE.lower(fileName), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.util.List<java.lang.String> sendGroupMessage(java.lang.String groupId, java.lang.String content, MessagePriority priority, java.lang.String replyToMsg) throws ProtocolException {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_group_message(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), FfiConverterString.INSTANCE.lower(content), FfiConverterOptionalTypeMessagePriority.INSTANCE.lower(priority), FfiConverterOptionalString.INSTANCE.lower(replyToMsg), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendMedia(java.lang.String recipient, java.util.List<java.lang.Byte> fileData, java.lang.String fileName, ContentType contentType, MediaMetadata mediaMetadata) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterSequenceByte.INSTANCE.lower(fileData), FfiConverterString.INSTANCE.lower(fileName), FfiConverterTypeContentType.INSTANCE.lower(contentType), FfiConverterOptionalTypeMediaMetadata.INSTANCE.lower(mediaMetadata), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendMediaRich(java.lang.String recipient, java.util.List<java.lang.Byte> fileData, java.lang.String fileName, ContentType contentType, MediaSendOptions options) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_media_rich(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterSequenceByte.INSTANCE.lower(fileData), FfiConverterString.INSTANCE.lower(fileName), FfiConverterTypeContentType.INSTANCE.lower(contentType), FfiConverterTypeMediaSendOptions.INSTANCE.lower(options), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendMessage(java.lang.String recipient, java.lang.String content, MessagePriority priority, java.lang.String replyToMsg) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterString.INSTANCE.lower(content), FfiConverterTypeMessagePriority.INSTANCE.lower(priority), FfiConverterOptionalString.INSTANCE.lower(replyToMsg), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendMessageRich(java.lang.String recipient, java.lang.String content, SendMessageOptions options) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_message_rich(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterString.INSTANCE.lower(content), FfiConverterTypeSendMessageOptions.INSTANCE.lower(options), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendPresenceUpdate(java.lang.String recipient, PresenceStatus status) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_presence_update(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterTypePresenceStatus.INSTANCE.lower(status), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendReadReceipt(java.lang.String recipient, java.util.List<java.lang.String> messageIds) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_read_receipt(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterSequenceString.INSTANCE.lower(messageIds), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public java.lang.String sendTypingIndicator(java.lang.String recipient, java.lang.String conversationId, boolean isTyping) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_send_typing_indicator(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(recipient), FfiConverterString.INSTANCE.lower(conversationId), FfiConverterBoolean.INSTANCE.lower(isTyping), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void setBatteryLevel(byte level) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_level(_uniffiHandle,
            level, _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void setBatteryState(byte level, boolean isCharging) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_battery_state(_uniffiHandle,
            level, FfiConverterBoolean.INSTANCE.lower(isCharging), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void setBleTransportCallback(BleTransportCallback callback)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_ble_transport_callback(_uniffiHandle,
            FfiConverterTypeBleTransportCallback.INSTANCE.lower(callback), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void setEventCallback(EventCallback callback)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_event_callback(_uniffiHandle,
            FfiConverterTypeEventCallback.INSTANCE.lower(callback), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void setMemberRole(java.lang.String groupId, java.lang.String userId, java.lang.String role) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_member_role(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(groupId), FfiConverterString.INSTANCE.lower(userId), FfiConverterString.INSTANCE.lower(role), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void setNostrTransportCallback(NostrTransportCallback callback)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_nostr_transport_callback(_uniffiHandle,
            FfiConverterTypeNostrTransportCallback.INSTANCE.lower(callback), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void setRelayPriority(RelayPriority priority) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_relay_priority(_uniffiHandle,
            FfiConverterTypeRelayPriority.INSTANCE.lower(priority), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void setReticulumTransportCallback(ReticulumTransportCallback callback)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_reticulum_transport_callback(_uniffiHandle,
            FfiConverterTypeReticulumTransportCallback.INSTANCE.lower(callback), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void setTelemetryEnabled(boolean enabled)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_telemetry_enabled(_uniffiHandle,
            FfiConverterBoolean.INSTANCE.lower(enabled), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void setWifiDirectTransportCallback(WifiDirectTransportCallback callback)  {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_set_wifi_direct_transport_callback(_uniffiHandle,
            FfiConverterTypeWifiDirectTransportCallback.INSTANCE.lower(callback), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public boolean shouldEscalateToWifi()  {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_should_escalate_to_wifi(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public java.util.List<java.lang.Byte> signData(java.util.List<java.lang.Byte> data) throws ProtocolException {
            try {
                return FfiConverterSequenceByte.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data(_allocator, _uniffiHandle,
            FfiConverterSequenceByte.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public byte[] signDataBytes(byte[] data) throws ProtocolException {
            try {
                return FfiConverterByteArray.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_sign_data_bytes(_allocator, _uniffiHandle,
            FfiConverterByteArray.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public void start() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_start(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void stop() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_stop(_uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public java.lang.String telemetryInstallId()  {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_install_id(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public TelemetryStats telemetryStats()  {
            try {
                return FfiConverterOptionalTypeTelemetryStats.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_telemetry_stats(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void unblockUser(java.lang.String userId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_unblock_user(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(userId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void updateAckConfig(AckConfig config) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_ack_config(_uniffiHandle,
            FfiConverterTypeAckConfig.INSTANCE.lower(config), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void updateDedupConfig(DedupConfig config) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dedup_config(_uniffiHandle,
            FfiConverterTypeDedupConfig.INSTANCE.lower(config), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void updateDorsConfig(DorsConfig config) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_dors_config(_uniffiHandle,
            FfiConverterTypeDorsConfig.INSTANCE.lower(config), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void updateRelayConfig(RelayConfig config) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_relay_config(_uniffiHandle,
            FfiConverterTypeRelayConfig.INSTANCE.lower(config), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void updateRetryConfig(RetryConfig config) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_retry_config(_uniffiHandle,
            FfiConverterTypeRetryConfig.INSTANCE.lower(config), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void updateTransportMetrics(TransportType transportType, TransportMetrics metrics) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_update_transport_metrics(_uniffiHandle,
            FfiConverterTypeTransportType.INSTANCE.lower(transportType), FfiConverterTypeTransportMetrics.INSTANCE.lower(metrics), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public boolean verifySignature(java.util.List<java.lang.Byte> publicKey, java.util.List<java.lang.Byte> data, java.util.List<java.lang.Byte> signature) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature(_uniffiHandle,
            FfiConverterSequenceByte.INSTANCE.lower(publicKey), FfiConverterSequenceByte.INSTANCE.lower(data), FfiConverterSequenceByte.INSTANCE.lower(signature), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public boolean verifySignatureBytes(byte[] publicKey, byte[] data, byte[] signature) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_verify_signature_bytes(_uniffiHandle,
            FfiConverterByteArray.INSTANCE.lower(publicKey), FfiConverterByteArray.INSTANCE.lower(data), FfiConverterByteArray.INSTANCE.lower(signature), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
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
    

  
    @Override
    public WifiDirectMessage wifiDirectGetNextMessage()  {
            try {
                return FfiConverterOptionalTypeWifiDirectMessage.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_get_next_message(_allocator, _uniffiHandle,
            _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    );
            } catch (java.lang.RuntimeException _uniffi_ex) {
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void wifiDirectMessageReceived(java.lang.String senderId, java.util.List<java.lang.Byte> data) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_message_received(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(senderId), FfiConverterSequenceByte.INSTANCE.lower(data), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void wifiDirectPeerConnected(java.lang.String peerId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_connected(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void wifiDirectPeerDisconnected(java.lang.String peerId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_peer_disconnected(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(peerId), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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
    

  
    @Override
    public void wifiDirectStatusChanged(boolean isConnected) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_offlineprotocol_wifi_direct_status_changed(_uniffiHandle,
            FfiConverterBoolean.INSTANCE.lower(isConnected), _status);
    });
    } catch (java.lang.RuntimeException _uniffi_ex) {
            throw _uniffi_ex;
        } catch (java.lang.Exception _uniffi_ex) {
            throw new java.lang.RuntimeException(_uniffi_ex);
        }
    })
    ;
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

