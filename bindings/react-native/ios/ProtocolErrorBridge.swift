//
//  ProtocolErrorBridge.swift
//
//  Maps typed protocol failures to the stable JS-visible rejection codes.
//  Keep in lockstep with the Android module's ProtocolErrorBridge.kt —
//  both mirror the UDL ProtocolError variant names (see types-uniffi.ts).
//

import Foundation

/// Maps a typed `ProtocolError` to its stable JS-visible rejection code,
/// or nil when the error has no typed mapping (callers fall back to their
/// legacy ERROR_* code with the message preserved).
func mapProtocolBridgeError(_ error: Error) -> (code: String, message: String)? {
    guard let protocolError = error as? ProtocolError else {
        return nil
    }

    switch protocolError {
    case .NoKeyPackage:
        return ("NoKeyPackage", "No key package available for recipient")
    case .SessionNotReady:
        return ("SessionNotReady", "Session not ready; establishment in progress")
    case .EncryptFailed:
        return ("EncryptFailed", "Message encryption failed")
    case .MediaTransferLimit:
        return ("MediaTransferLimit", "Too many concurrent media transfers to this recipient; retry after an active transfer completes")
    case let .SendFailed(message):
        return ("SendFailed", message)
    case let .InvalidState(message):
        return ("InvalidState", message)
    case let .MlsNotInitialized(message):
        return ("MlsNotInitialized", message)
    case let .TransportError(message):
        return ("TransportError", message)
    case let .SerializationError(message):
        return ("SerializationError", message)
    case let .ServiceError(message):
        return ("ServiceError", message)
    case let .GroupNotFound(message):
        return ("GroupNotFound", message)
    case let .PermissionDenied(message):
        return ("PermissionDenied", message)
    case let .InvalidArgument(message):
        return ("InvalidArgument", message)
    default:
        return nil
    }
}
