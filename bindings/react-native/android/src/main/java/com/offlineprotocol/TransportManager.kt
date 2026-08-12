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
     * Called when the transport emits a diagnostic message
     */
    fun onTransportDiagnostic(manager: TransportManager, level: String, message: String, context: Map<String, Any?>)
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
     *
     * ## Threading
     *
     * An implementation must not run UniFFI calls on the caller's thread.
     * Every call into the core serialises on one global protocol mutex, held
     * across MLS work and platform keystore access, so a lifecycle method that
     * calls the protocol inline hands those waits to whoever called it — and
     * the callers here include the app's main thread. Confine the work
     * instead; [TransportConfinement] is what the shipped transports use.
     */
    fun start()

    /**
     * Stops the transport
     *
     * Same threading obligation as [start].
     */
    fun stop()

    /**
     * Pauses the transport (for background mode)
     *
     * The default delegates to [stop] and therefore inherits the caller's
     * thread. Every transport in this module overrides it, so nothing reaches
     * this body today — but a new transport that does not override is exactly
     * how lifecycle FFI creeps back onto main. Override both, or confine
     * inside [start] and [stop].
     *
     * Confining does not mean posting and returning. This must have *taken
     * effect* by the time it returns: `OfflineProtocolModule.pause` pauses
     * every transport and then the core, and that order is what bounds the
     * window in which a paused transport can still re-enter UniFFI. An
     * implementation that hands the work to its handler and returns has paused
     * nothing yet, so the core can pause underneath it. Wait on the
     * confinement — the module's caller is a React Native background thread,
     * where [TransportConfinement.runSync] does not bound the wait.
     */
    fun pause() {
        // Default implementation: stop the transport
        stop()
    }

    /**
     * Resumes the transport from paused state
     *
     * Same caveat as [pause].
     */
    fun resume() {
        // Default implementation: try to start the transport
        try {
            start()
        } catch (e: Exception) {
            // Ignore errors on resume
        }
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

