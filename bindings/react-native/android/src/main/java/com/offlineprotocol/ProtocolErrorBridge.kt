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
        else -> null
    }
}
