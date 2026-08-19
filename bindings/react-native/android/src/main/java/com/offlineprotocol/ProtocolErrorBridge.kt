package com.offlineprotocol

import uniffi.offline_protocol.ProtocolException

/**
 * A typed protocol failure surfaced to JS as a stable rejection code.
 * Codes mirror the UDL ProtocolError variant names (see the ProtocolError
 * enum in crates/offline-protocol-uniffi/src/offline_protocol.udl).
 */
internal data class BridgeProtocolError(
    val code: String,
    val message: String
)

/**
 * Maps a typed [ProtocolException] to its stable JS-visible rejection code,
 * or null when the error has no typed mapping (callers fall back to their
 * legacy ERROR_* code with the message preserved).
 */
internal fun mapProtocolBridgeError(error: Throwable): BridgeProtocolError? {
    return when (error) {
        is ProtocolException.NoKeyPackage -> BridgeProtocolError(
            code = "NoKeyPackage",
            message = "No key package available for recipient"
        )
        is ProtocolException.SessionNotReady -> BridgeProtocolError(
            code = "SessionNotReady",
            message = "Session not ready; establishment in progress"
        )
        is ProtocolException.EncryptFailed -> BridgeProtocolError(
            code = "EncryptFailed",
            message = "Message encryption failed"
        )
        is ProtocolException.MediaTransferLimit -> BridgeProtocolError(
            code = "MediaTransferLimit",
            message = "Too many concurrent media transfers to this recipient; retry after an active transfer completes"
        )
        is ProtocolException.SendFailed -> BridgeProtocolError(
            code = "SendFailed",
            message = error.message ?: "Send failed"
        )
        is ProtocolException.InvalidState -> BridgeProtocolError(
            code = "InvalidState",
            message = error.message ?: "Operation rejected by current state"
        )
        // Also transient, and distinct from InvalidState because the retry is
        // gated on something the app itself controls: resolveUsername refuses a
        // lookup while the protocol is stopped or paused, since nothing would
        // pump relays or sweep its deadline. Without this arm it fell to the
        // caller's fallback code, which for that method is InvalidArgument —
        // telling the app the *name* was unusable and that retrying could never
        // help, when start() is all that was missing.
        is ProtocolException.NotStarted -> BridgeProtocolError(
            code = "NotStarted",
            message = error.message ?: "Protocol not started"
        )
        // Distinct from InvalidState on purpose: this one says the
        // *configuration* forbids the call, so retrying it unchanged can never
        // succeed, while InvalidState is transient and a retry is exactly
        // right. resolveUsername raises both, and an app that cannot tell them
        // apart either spins forever on a config error or gives up on a queue
        // that was about to drain.
        is ProtocolException.InvalidConfiguration -> BridgeProtocolError(
            code = "InvalidConfiguration",
            message = error.message ?: "Operation rejected by current configuration"
        )
        is ProtocolException.MlsNotInitialized -> BridgeProtocolError(
            code = "MlsNotInitialized",
            message = error.message ?: "MLS not initialized"
        )
        is ProtocolException.TransportException -> BridgeProtocolError(
            code = "TransportError",
            message = error.message ?: "Transport error"
        )
        is ProtocolException.SerializationException -> BridgeProtocolError(
            code = "SerializationError",
            message = error.message ?: "Serialization error"
        )
        is ProtocolException.ServiceException -> BridgeProtocolError(
            code = "ServiceError",
            message = error.message ?: "Service error"
        )
        is ProtocolException.GroupNotFound -> BridgeProtocolError(
            code = "GroupNotFound",
            message = error.message ?: "Group not found"
        )
        is ProtocolException.PermissionDenied -> BridgeProtocolError(
            code = "PermissionDenied",
            message = error.message ?: "Permission denied"
        )
        is ProtocolException.InvalidArgument -> BridgeProtocolError(
            code = "InvalidArgument",
            message = error.message ?: "Invalid argument"
        )
        // Data-layer conditions, each distinct because the app's next move
        // differs. DataDisabled and DataStorageUnavailable are setup mistakes
        // that no retry fixes; DocTooLarge is recoverable by deleting content
        // (deletions keep working while growth is refused); DataCorrupted is
        // permanent for that document. Without these arms they would all land
        // on the caller's fallback code and read as one undifferentiated
        // failure.
        is ProtocolException.DataDisabled -> BridgeProtocolError(
            code = "DataDisabled",
            message = error.message ?: "Data layer is disabled"
        )
        is ProtocolException.DataStorageUnavailable -> BridgeProtocolError(
            code = "DataStorageUnavailable",
            message = error.message ?: "Data layer has no storage; initializeMls must run first"
        )
        is ProtocolException.DocTooLarge -> BridgeProtocolError(
            code = "DocTooLarge",
            message = error.message ?: "Document is over the size cap"
        )
        is ProtocolException.DataCorrupted -> BridgeProtocolError(
            code = "DataCorrupted",
            message = error.message ?: "Document data is corrupt"
        )
        else -> null
    }
}
