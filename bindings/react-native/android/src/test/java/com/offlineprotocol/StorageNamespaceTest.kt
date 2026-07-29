package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class StorageNamespaceTest {
    @Test
    fun accountNamespaceIsStableAndOpaque() {
        assertEquals(
            "account-814873e0cbdb2a1f25f14b31625e7f904cf9923e55b415b91ca4b29b210c12a1",
            StorageNamespace.account("test-app", "test-user-1")
        )
    }

    @Test
    fun accountNamespaceSeparatesAccounts() {
        assertNotEquals(
            StorageNamespace.account("chat", "alice"),
            StorageNamespace.account("chat", "bob")
        )
        assertNotEquals(
            StorageNamespace.account("chat", "alice"),
            StorageNamespace.account("other-chat", "alice")
        )
    }

    @Test
    fun generatedNamespacesPassValidation() {
        val namespace = StorageNamespace.account("chat", "alice")
        assertEquals(namespace, StorageNamespace.requireAccount(namespace))
    }

    /**
     * A namespace becomes a directory component and a preferences file name, so
     * anything that could escape or collide must be refused at the door.
     */
    @Test
    fun malformedNamespacesAreRefused() {
        val rejected = listOf(
            "",
            "account-",
            "../../other-account",
            "account-" + "a".repeat(63),
            "account-" + "a".repeat(65),
            "account-" + "A".repeat(64),
            "account-" + "g".repeat(64)
        )
        for (value in rejected) {
            assertThrows(IllegalArgumentException::class.java) {
                StorageNamespace.requireAccount(value)
            }
        }
    }
}
