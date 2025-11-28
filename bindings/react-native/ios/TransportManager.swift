//
// TransportManager.swift
// OfflineProtocol
//
// Transport manager protocol for extensible transport implementations
//

import Foundation

/// Transport lifecycle states
public enum TransportState {
    case unavailable
    case available
    case starting
    case running
    case stopping
    case stopped
}

/// Delegate for transport events
public protocol TransportManagerDelegate: AnyObject {
    /// Called when transport state changes
    func transportManager(_ manager: TransportManager, didChangeState state: TransportState)
    
    /// Called when transport encounters an error
    func transportManager(_ manager: TransportManager, didEncounterError error: Error)
    
    /// Called when transport metrics are updated
    func transportManager(_ manager: TransportManager, didUpdateMetrics metrics: [String: Any])

    /// Called when the transport emits a diagnostic message
    func transportManager(_ manager: TransportManager, didEmitDiagnostic level: String, message: String, context: [String: Any])
}

/// Base protocol for all transport implementations
/// This allows for extensible transport system supporting BLE, WiFi Direct, and Internet
public protocol TransportManager: AnyObject {
    /// Unique transport identifier
    var transportId: String { get }
    
    /// Human-readable transport name
    var transportName: String { get }
    
    /// Current transport state
    var state: TransportState { get }
    
    /// Delegate for transport events
    var delegate: TransportManagerDelegate? { get set }
    
    /// Checks if the transport is available on this device
    /// - Returns: true if transport hardware/capabilities are available
    func isAvailable() -> Bool
    
    /// Starts the transport
    /// - Throws: TransportError if start fails
    func start() throws
    
    /// Stops the transport
    func stop()
    
    /// Pauses the transport (for background mode)
    func pause()
    
    /// Resumes the transport from paused state
    func resume()
    
    /// Gets current transport metrics
    /// - Returns: Dictionary of metric name to value
    func getMetrics() -> [String: Any]
}

/// Transport-specific errors
public enum TransportError: LocalizedError {
    case notAvailable(String)
    case permissionDenied(String)
    case startFailed(String)
    case alreadyRunning
    case notRunning
    case invalidState(String)
    case platformError(Error)
    
    public var errorDescription: String? {
        switch self {
        case .notAvailable(let reason):
            return "Transport not available: \(reason)"
        case .permissionDenied(let permission):
            return "Permission denied: \(permission)"
        case .startFailed(let reason):
            return "Failed to start transport: \(reason)"
        case .alreadyRunning:
            return "Transport is already running"
        case .notRunning:
            return "Transport is not running"
        case .invalidState(let message):
            return "Invalid state: \(message)"
        case .platformError(let error):
            return "Platform error: \(error.localizedDescription)"
        }
    }
}

/// Extension for default implementations
extension TransportManager {
    public func pause() {
        // Default implementation: stop the transport
        stop()
    }
    
    public func resume() {
        // Default implementation: try to start the transport
        try? start()
    }
    
    public func getMetrics() -> [String: Any] {
        // Default implementation: return empty metrics
        return [:]
    }
}

