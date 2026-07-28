package com.offlineprotocol

import java.security.MessageDigest

/**
 * Produces a stable, opaque namespace for one protocol account.
 *
 * Keep the domain separator and encoding in sync with the iOS and Python
 * bindings so every built-in provider follows the same isolation contract.
 */
internal object StorageNamespace {
    private const val DOMAIN = "offline-protocol-storage-v1"
    private const val HEX = "0123456789abcdef"
    private val accountPattern = Regex("account-[0-9a-f]{64}")

    fun account(appId: String, userId: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(
            "$DOMAIN\u0000$appId\u0000$userId".toByteArray(Charsets.UTF_8)
        )
        return buildString(8 + digest.size * 2) {
            append("account-")
            digest.forEach { byte ->
                val value = byte.toInt() and 0xff
                append(HEX[value ushr 4])
                append(HEX[value and 0x0f])
            }
        }
    }

    fun requireAccount(value: String): String {
        require(accountPattern.matches(value)) {
            "Invalid protocol storage account namespace"
        }
        return value
    }
}
