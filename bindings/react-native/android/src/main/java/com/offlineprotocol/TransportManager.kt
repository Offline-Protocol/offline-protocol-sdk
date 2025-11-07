package com.offlineprotocol

/**
 * Transport lifecycle states
 */
enum class TransportState {
    UNAVAILABLE,
    AVAILABLE,
    STARTING,
    RUNNING,
    STOPPING,
    STOPPED
}

/**
 * Listener interface for transport events
 */
interface TransportManagerListener {
    /**
     * Called when transport state changes
     */
    fun onTransportStateChanged(manager: TransportManager, state: TransportState)
    
    /**
     * Called when transport encounters an error
     */
    fun onTransportError(manager: TransportManager, error: Throwable)
    
    /**
     * Called when transport metrics are updated
     */
    fun onTransportMetricsUpdated(manager: TransportManager, metrics: Map<String, Any>)
}

/**
 * Base interface for all transport implementations
 * This allows for extensible transport system supporting BLE, WiFi Direct, and Internet
 */
interface TransportManager {
    /**
     * Unique transport identifier
     */
    val transportId: String
    
    /**
     * Human-readable transport name
     */
    val transportName: String
    
    /**
     * Current transport state
     */
    val state: TransportState
    
    /**
     * Listener for transport events
     */
    var listener: TransportManagerListener?
    
    /**
     * Checks if the transport is available on this device
     * @return true if transport hardware/capabilities are available
     */
    fun isAvailable(): Boolean
    
    /**
     * Starts the transport
     * @throws TransportException if start fails
     */
    fun start()
    
    /**
     * Stops the transport
     */
    fun stop()
    
    /**
     * Pauses the transport (for background mode)
     */
    fun pause() {
        // Default implementation: stop the transport
        stop()
    }
    
    /**
     * Resumes the transport from paused state
     */
    fun resume() {
        // Default implementation: try to start the transport
        try {
            start()
        } catch (e: Exception) {
            // Ignore errors on resume
        }
    }
    
    /**
     * Gets current transport metrics
     * @return Map of metric name to value
     */
    fun getMetrics(): Map<String, Any> {
        // Default implementation: return empty metrics
        return emptyMap()
    }
}

/**
 * Transport-specific exceptions
 */
sealed class TransportException(message: String, cause: Throwable? = null) : Exception(message, cause) {
    class NotAvailable(reason: String) : TransportException("Transport not available: $reason")
    class PermissionDenied(permission: String) : TransportException("Permission denied: $permission")
    class StartFailed(reason: String, cause: Throwable? = null) : TransportException("Failed to start transport: $reason", cause)
    class AlreadyRunning : TransportException("Transport is already running")
    class NotRunning : TransportException("Transport is not running")
    class InvalidState(message: String) : TransportException("Invalid state: $message")
    class PlatformError(cause: Throwable) : TransportException("Platform error: ${cause.message}", cause)
}

