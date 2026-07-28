package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
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

    @Test(expected = IllegalArgumentException::class)
    fun accountNamespaceRejectsPathComponents() {
        StorageNamespace.requireAccount("../../other-account")
    }
}
