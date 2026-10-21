package com.offlineprotocol.sdk;

public class DataStore implements AutoCloseable, DataStoreInterface {
  protected long handle;
  protected UniffiCleaner.Cleanable cleanable;

  private java.util.concurrent.atomic.AtomicBoolean wasDestroyed = new java.util.concurrent.atomic.AtomicBoolean(false);
  private java.util.concurrent.atomic.AtomicLong callCounter = new java.util.concurrent.atomic.AtomicLong(1);

  /**
   * Internal constructor to wrap a raw handle from FFI.
   * The UniffiWithHandle marker disambiguates this from other constructors.
   */
  public DataStore(UniffiWithHandle phantom, long handle) {
    this.handle = handle;
    this.cleanable = UniffiLib.CLEANER.register(this, new UniffiCleanAction(handle));
  }

  /**
   * This constructor can be used to instantiate a fake object. Only used for tests. Any
   * attempt to actually use an object constructed this way will fail as there is no
   * connected Rust object.
   */
  public DataStore(NoHandle noHandle) {
    this.handle = 0L;
    this.cleanable = null;
  }

  
  public DataStore(OfflineProtocol protocol) throws ProtocolException{
    this(UniffiWithHandle.INSTANCE, (long)
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_constructor_datastore_new(
            FfiConverterTypeOfflineProtocol.INSTANCE.lower(protocol), _status);
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
        throw new java.lang.IllegalStateException("DataStore object has already been destroyed");
      }
      if (c == java.lang.Long.MAX_VALUE) {
        throw new java.lang.IllegalStateException("DataStore call counter would overflow");
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
          UniffiLib.uniffi_offline_protocol_uniffi_fn_free_datastore(handle, status);
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
      return UniffiLib.uniffi_offline_protocol_uniffi_fn_clone_datastore(handle, status);
    });
  }

  
    @Override
    public java.lang.String attachmentHash(byte[] data)  {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCall( (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_attachment_hash(_allocator, _uniffiHandle,
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
                
                
                if (InternalException.class.isInstance(_uniffi_ex.getCause())) {
                    throw (InternalException)_uniffi_ex.getCause();
                }
                throw _uniffi_ex;
            }
    }
    

  
    @Override
    public void counterIncrement(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, double amount) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_counter_increment(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), amount, _status);
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
    public double counterValue(java.lang.String spaceId, java.lang.String docId, java.lang.String collection) throws ProtocolException {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithErrorDouble(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_counter_value(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), _status);
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
    public void createDoc(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_create_doc(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public void declineAttachment(java.lang.String spaceId, java.lang.String peerId, java.lang.String hash) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_decline_attachment(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(peerId), FfiConverterString.INSTANCE.lower(hash), _status);
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
    public void deleteDoc(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_delete_doc(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public java.lang.String docJson(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_doc_json(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public long docSize(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithErrorLong(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_doc_size(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public byte[] exportRaw(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                return FfiConverterByteArray.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_export_raw(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public void fetchAttachment(java.lang.String spaceId, java.lang.String hash) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(hash), _status);
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
    public void fetchAttachmentFrom(java.lang.String spaceId, java.lang.String peerId, java.lang.String hash) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_fetch_attachment_from(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(peerId), FfiConverterString.INSTANCE.lower(hash), _status);
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
    public void flush(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_flush(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public void flushAll() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_flush_all(_uniffiHandle,
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
    public void listDelete(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, int index, int count) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_list_delete(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), index, count, _status);
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
    public java.util.List<java.lang.String> listDocs(java.lang.String spaceId) throws ProtocolException {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_list_docs(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), _status);
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
    public int listLen(java.lang.String spaceId, java.lang.String docId, java.lang.String collection) throws ProtocolException {
            try {
                return 
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithErrorInt(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_list_len(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), _status);
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
    public void listPush(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String valueJson) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_list_push(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), FfiConverterString.INSTANCE.lower(valueJson), _status);
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
    public java.util.List<java.lang.String> listSpaces() throws ProtocolException {
            try {
                return FfiConverterSequenceString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_list_spaces(_allocator, _uniffiHandle,
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
    public void mapDelete(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String key) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_map_delete(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), FfiConverterString.INSTANCE.lower(key), _status);
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
    public java.lang.String mapGetJson(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String key) throws ProtocolException {
            try {
                return FfiConverterOptionalString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_map_get_json(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), FfiConverterString.INSTANCE.lower(key), _status);
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
    public void mapSet(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, java.lang.String key, java.lang.String valueJson) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_map_set(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), FfiConverterString.INSTANCE.lower(key), FfiConverterString.INSTANCE.lower(valueJson), _status);
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
    public void provideAttachment(java.lang.String spaceId, java.lang.String peerId, java.lang.String hash, byte[] data) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_provide_attachment(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(peerId), FfiConverterString.INSTANCE.lower(hash), FfiConverterByteArray.INSTANCE.lower(data), _status);
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
    public void removeDoc(java.lang.String spaceId, java.lang.String docId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_remove_doc(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), _status);
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
    public void removeSpace(java.lang.String spaceId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_remove_space(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), _status);
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
    public void setInterest(java.lang.String spaceId, java.util.List<java.lang.String> patterns) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_set_interest(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterSequenceString.INSTANCE.lower(patterns), _status);
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
    public void textDelete(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, int position, int count) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_text_delete(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), position, count, _status);
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
    public void textInsert(java.lang.String spaceId, java.lang.String docId, java.lang.String collection, int position, java.lang.String text) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_text_insert(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), position, FfiConverterString.INSTANCE.lower(text), _status);
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
    public java.lang.String textValue(java.lang.String spaceId, java.lang.String docId, java.lang.String collection) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_text_value(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(spaceId), FfiConverterString.INSTANCE.lower(docId), FfiConverterString.INSTANCE.lower(collection), _status);
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
    public void wipeAll() throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_datastore_wipe_all(_uniffiHandle,
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
    

  

  


  public static DataStore withStorage(OfflineProtocol protocol, ProtocolStateStorageProvider storage) throws ProtocolException {
            try {
                return FfiConverterTypeDataStore.INSTANCE.lift(
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_constructor_datastore_with_storage(
            FfiConverterTypeOfflineProtocol.INSTANCE.lower(protocol), FfiConverterTypeProtocolStateStorageProvider.INSTANCE.lower(storage), _status);
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

