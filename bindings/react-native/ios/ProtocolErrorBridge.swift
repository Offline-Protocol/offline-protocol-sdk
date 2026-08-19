//
//  ProtocolErrorBridge.swift
//
//  Maps typed protocol failures to the stable JS-visible rejection codes.
//  Keep in lockstep with the Android module's ProtocolErrorBridge.kt —
//  both mirror the UDL ProtocolError variant names (see the ProtocolError
//  enum in crates/offline-protocol-uniffi/src/offline_protocol.udl).
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
    // Also transient, and distinct from InvalidState because the retry is
    // gated on something the app itself controls: `resolveUsername` refuses a
    // lookup while the protocol is stopped or paused, since nothing would pump
    // relays or sweep its deadline. Without this arm it fell to the caller's
    // fallback code, which for that method is InvalidArgument — telling the app
    // the *name* was unusable and that retrying could never help, when
    // `start()` is all that was missing.
    case let .NotStarted(message):
        return ("NotStarted", message)
    // Distinct from InvalidState on purpose: this one says the *configuration*
    // forbids the call, so retrying it unchanged can never succeed, while
    // InvalidState is transient and a retry is exactly right. `resolveUsername`
    // raises both, and an app that cannot tell them apart either spins forever
    // on a config error or gives up on a queue that was about to drain.
    case let .InvalidConfiguration(message):
        return ("InvalidConfiguration", message)
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
    // Data-layer conditions, each distinct because the app's next move
    // differs. DataDisabled and DataStorageUnavailable are setup mistakes that
    // no retry fixes; DocTooLarge is recoverable by deleting content
    // (deletions keep working while growth is refused); DataCorrupted is
    // permanent for that document. Without these cases they would all fall to
    // the caller's fallback code and read as one undifferentiated failure.
    case .DataDisabled:
        return ("DataDisabled", "Data layer is disabled")
    case .DataStorageUnavailable:
        return ("DataStorageUnavailable", "Data layer has no storage; initializeMls must run first")
    case let .DocTooLarge(message):
        return ("DocTooLarge", message)
    case let .DataCorrupted(message):
        return ("DataCorrupted", message)
    default:
        return nil
    }
}
