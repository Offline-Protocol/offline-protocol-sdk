package com.offlineprotocol.sdk;

public class MeshServices implements AutoCloseable, MeshServicesInterface {
  protected long handle;
  protected UniffiCleaner.Cleanable cleanable;

  private java.util.concurrent.atomic.AtomicBoolean wasDestroyed = new java.util.concurrent.atomic.AtomicBoolean(false);
  private java.util.concurrent.atomic.AtomicLong callCounter = new java.util.concurrent.atomic.AtomicLong(1);

  /**
   * Internal constructor to wrap a raw handle from FFI.
   * The UniffiWithHandle marker disambiguates this from other constructors.
   */
  public MeshServices(UniffiWithHandle phantom, long handle) {
    this.handle = handle;
    this.cleanable = UniffiLib.CLEANER.register(this, new UniffiCleanAction(handle));
  }

  /**
   * This constructor can be used to instantiate a fake object. Only used for tests. Any
   * attempt to actually use an object constructed this way will fail as there is no
   * connected Rust object.
   */
  public MeshServices(NoHandle noHandle) {
    this.handle = 0L;
    this.cleanable = null;
  }

  
  public MeshServices(OfflineProtocol protocol) throws ProtocolException{
    this(UniffiWithHandle.INSTANCE, (long)
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_constructor_meshservices_new(
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
        throw new java.lang.IllegalStateException("MeshServices object has already been destroyed");
      }
      if (c == java.lang.Long.MAX_VALUE) {
        throw new java.lang.IllegalStateException("MeshServices call counter would overflow");
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
          UniffiLib.uniffi_offline_protocol_uniffi_fn_free_meshservices(handle, status);
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
      return UniffiLib.uniffi_offline_protocol_uniffi_fn_clone_meshservices(handle, status);
    });
  }

  
    @Override
    public void acceptServiceSubscription(java.lang.String subId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_accept_service_subscription(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(subId), _status);
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
    public void clearServiceTopic(java.lang.String serviceId, java.lang.String topic) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_clear_service_topic(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(topic), _status);
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
    public java.lang.String discoverServices(java.lang.String serviceId) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_discover_services(_allocator, _uniffiHandle,
            FfiConverterOptionalString.INSTANCE.lower(serviceId), _status);
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
    public void endServiceSubscription(java.lang.String subId, java.lang.String reason) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_end_service_subscription(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(subId), FfiConverterString.INSTANCE.lower(reason), _status);
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
    public ServicePublishReport publishServiceUpdate(java.lang.String serviceId, java.lang.String topic, java.lang.String key, java.lang.String body) throws ProtocolException {
            try {
                return FfiConverterTypeServicePublishReport.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_publish_service_update(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(topic), FfiConverterOptionalString.INSTANCE.lower(key), FfiConverterString.INSTANCE.lower(body), _status);
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
    public void registerService(java.lang.String serviceId, java.lang.String version, java.util.Map<java.lang.String, java.lang.String> capabilities) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_register_service(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(version), FfiConverterMapStringString.INSTANCE.lower(capabilities), _status);
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
    public void rejectServiceSubscription(java.lang.String subId, java.lang.String reason) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_reject_service_subscription(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(subId), FfiConverterString.INSTANCE.lower(reason), _status);
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
    public java.lang.String respondToServiceRequest(java.lang.String requestId, java.lang.String requester, java.lang.String serviceId, java.lang.String status, java.lang.String body) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_respond_to_service_request(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(requestId), FfiConverterString.INSTANCE.lower(requester), FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(status), FfiConverterString.INSTANCE.lower(body), _status);
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
    public void resyncServiceSubscription(java.lang.String subId) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_resync_service_subscription(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(subId), _status);
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
    public java.lang.String sendServiceRequest(java.lang.String provider, java.lang.String serviceId, java.lang.String method, java.lang.String body) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_send_service_request(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(provider), FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(method), FfiConverterString.INSTANCE.lower(body), _status);
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
    public java.util.List<ServiceSubscriberInfo> serviceSubscribers(java.lang.String serviceId) throws ProtocolException {
            try {
                return FfiConverterSequenceTypeServiceSubscriberInfo.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_service_subscribers(_allocator, _uniffiHandle,
            FfiConverterOptionalString.INSTANCE.lower(serviceId), _status);
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
    public java.util.List<ServiceSubscriptionInfo> serviceSubscriptions() throws ProtocolException {
            try {
                return FfiConverterSequenceTypeServiceSubscriptionInfo.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_service_subscriptions(_allocator, _uniffiHandle,
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
    public void setServiceSubscriptionPolicy(java.lang.String serviceId, SubscriptionPolicyConfig policy) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_set_service_subscription_policy(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(serviceId), FfiConverterTypeSubscriptionPolicyConfig.INSTANCE.lower(policy), _status);
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
    public void setServiceTopicCheckpoint(java.lang.String serviceId, java.lang.String topic, java.lang.String body) throws ProtocolException {
            try {
                
    callWithHandle(_uniffiHandle -> {
        try {
    
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_set_service_topic_checkpoint(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(topic), FfiConverterString.INSTANCE.lower(body), _status);
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
    public java.lang.String subscribeService(java.lang.String provider, java.lang.String serviceId, java.lang.String topic, SubscriptionMode mode, java.lang.String params) throws ProtocolException {
            try {
                return FfiConverterString.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_subscribe_service(_allocator, _uniffiHandle,
            FfiConverterString.INSTANCE.lower(provider), FfiConverterString.INSTANCE.lower(serviceId), FfiConverterString.INSTANCE.lower(topic), FfiConverterTypeSubscriptionMode.INSTANCE.lower(mode), FfiConverterString.INSTANCE.lower(params), _status);
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
    public boolean unregisterService(java.lang.String serviceId) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_unregister_service(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(serviceId), _status);
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
    public boolean unsubscribeService(java.lang.String subId) throws ProtocolException {
            try {
                return FfiConverterBoolean.INSTANCE.lift(
    callWithHandle(_uniffiHandle -> {
        try {
    
            return
    UniffiHelpers.uniffiRustCallWithError(new ProtocolExceptionErrorHandler(), (_allocator, _status) -> {
        return UniffiLib.uniffi_offline_protocol_uniffi_fn_method_meshservices_unsubscribe_service(_uniffiHandle,
            FfiConverterString.INSTANCE.lower(subId), _status);
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
    

  

  


  
}

