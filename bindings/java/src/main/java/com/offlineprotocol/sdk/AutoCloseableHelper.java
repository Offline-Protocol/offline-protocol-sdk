package com.offlineprotocol.sdk;


import java.util.List;
import java.util.Map;
import java.util.stream.Stream;

public interface AutoCloseableHelper {
    // Use java.lang.Object to avoid conflict with user-defined Object types
    static void close(java.lang.Object... args) {
        Stream
            .of(args)
            .forEach(obj -> {
                // this is all to avoid the problem reported in uniffi-rs#2467
                if (obj instanceof AutoCloseable) {
                    try {
                        ((AutoCloseable) obj).close();
                    } catch (java.lang.Exception ex) {
                        throw new RuntimeException(ex);
                    }
                }
                if (obj instanceof List<?>) {
                    for (int i = 0; i < ((List) obj).size(); i++) {
                        java.lang.Object element = ((List) obj).get(i);
                        if (element instanceof AutoCloseable) {
                            try {
                                ((AutoCloseable) element).close();
                            } catch (java.lang.Exception ex) {
                                throw new RuntimeException(ex);
                            }
                        }
                    }
                }
                if (obj instanceof Map<?, ?>) {
                    for (var value : ((Map) obj).values()) {
                        if (value instanceof AutoCloseable) {
                            try {
                                ((AutoCloseable) value).close();
                            } catch (java.lang.Exception ex) {
                                throw new RuntimeException(ex);
                            }
                        }
                    }
                }
                if (obj instanceof Iterable<?>) {
                    for (var value : ((Iterable) obj)) {
                        if (value instanceof AutoCloseable) {
                            try {
                                ((AutoCloseable) value).close();
                            } catch (java.lang.Exception ex) {
                                throw new RuntimeException(ex);
                            }
                        }
                    }
                }
            });
    }
}
