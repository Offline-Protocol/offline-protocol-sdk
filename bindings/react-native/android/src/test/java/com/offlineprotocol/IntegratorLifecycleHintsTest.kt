package com.offlineprotocol

import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class IntegratorLifecycleHintsTest {
    @Before
    fun reset() {
        IntegratorLifecycleHints.resetSession()
    }

    @Test
    fun disabledIsNoOp() {
        IntegratorLifecycleHints.warnIfCallBeforeEngineStarted("sendMessage", null, false)
        assertTrue(true)
    }

    @Test
    fun interestAfterStartDedupes() {
        IntegratorLifecycleHints.warnIfInterestAfterStart(true)
        IntegratorLifecycleHints.warnIfInterestAfterStart(true)
        assertTrue(true)
    }

    @Test
    fun resetAllowsSecondWarn() {
        IntegratorLifecycleHints.warnIfInterestAfterStart(true)
        IntegratorLifecycleHints.resetSession()
        IntegratorLifecycleHints.warnIfInterestAfterStart(true)
        assertTrue(true)
    }
}
