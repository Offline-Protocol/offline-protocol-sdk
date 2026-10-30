package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.offline_protocol.ServiceSubscriberInfo
import uniffi.offline_protocol.ServiceSubscriptionInfo
import uniffi.offline_protocol.SubscriptionMode
import uniffi.offline_protocol.SubscriptionPolicyKind
import uniffi.offline_protocol.SubscriptionRulesFallback
import uniffi.offline_protocol.SubscriptionState

/**
 * The subscription records cross to JavaScript and Dart as plain maps. Every
 * enum in them must be the lowercase string the TypeScript types and the
 * engine's events use: a raw enum reaches React Native as "PENDING" and makes
 * Flutter's StandardMessageCodec throw. Flutter's Android handler delegates to
 * the same bridge, so this covers both.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ServiceSubscriptionBridgeTest {

    @Test
    fun aSubscriptionCrossesWithLowercaseModeAndState() {
        val map = ProtocolEngineHostBridge.subscriptionInfoMap(
            ServiceSubscriptionInfo(
                subId = "sub-1",
                provider = "bob",
                serviceId = "fleet.v1",
                topic = "rooms/*",
                mode = SubscriptionMode.LATEST,
                state = SubscriptionState.PAUSED,
                keys = listOf("temp"),
                topics = listOf("rooms/a"),
            ),
        )
        assertEquals("latest", map["mode"])
        assertEquals("paused", map["state"])
        assertEquals(listOf("temp"), map["keys"])
        assertEquals(listOf("rooms/a"), map["topics"])
    }

    @Test
    fun aSubscriberCrossesWithLowercaseModeAndState() {
        val map = ProtocolEngineHostBridge.subscriberInfoMap(
            ServiceSubscriberInfo(
                subId = "sub-2",
                subscriber = "alice",
                serviceId = "fleet.v1",
                topic = "doors",
                mode = SubscriptionMode.LOG,
                state = SubscriptionState.ACTIVE,
                params = "{}",
                keys = emptyList(),
            ),
        )
        assertEquals("log", map["mode"])
        assertEquals("active", map["state"])
        assertEquals("{}", map["params"])
    }

    @Test
    fun everyStateHasTheLowercaseNameTheEventsUse() {
        val names = SubscriptionState.values().map { state ->
            ProtocolEngineHostBridge.subscriptionInfoMap(
                ServiceSubscriptionInfo("s", "p", "svc", "t", SubscriptionMode.LOG, state, emptyList(), emptyList()),
            )["state"]
        }
        assertEquals(listOf("pending", "active", "paused", "ended"), names)
    }

    @Test
    fun aRulesPolicyIsParsedFromItsJson() {
        val policy = ProtocolEngineHostBridge.subscriptionPolicyFromJson(
            """{"kind":"rules","allowGroups":["group:1"],"fallback":"reject"}""",
        )
        assertEquals(SubscriptionPolicyKind.RULES, policy.kind)
        assertEquals(emptyList<String>(), policy.allowPeers)
        assertEquals(listOf("group:1"), policy.allowGroups)
        assertEquals(SubscriptionRulesFallback.REJECT, policy.rulesFallback)
    }

    @Test
    fun aPolicyListThatIsNotAllStringsIsRefused() {
        val error = assertThrows(IllegalArgumentException::class.java) {
            ProtocolEngineHostBridge.subscriptionPolicyFromJson(
                """{"kind":"rules","allowPeers":["alice",7]}""",
            )
        }
        assertEquals("'allowPeers' must be a list of strings", error.message)
    }

    @Test
    fun anUnknownPolicyKindIsRefused() {
        assertThrows(IllegalArgumentException::class.java) {
            ProtocolEngineHostBridge.subscriptionPolicyFromJson("""{"kind":"open"}""")
        }
    }

    @Test
    fun keysJsonThatIsNotAllStringsIsRefused() {
        assertEquals(
            listOf("temp", "pos.*"),
            ProtocolEngineHostBridge.subscriptionKeys(mapOf("keysJson" to """["temp","pos.*"]""")),
        )
        assertEquals(listOf("a"), ProtocolEngineHostBridge.subscriptionKeys(mapOf("keys" to listOf("a"))))
        assertThrows(IllegalArgumentException::class.java) {
            ProtocolEngineHostBridge.subscriptionKeys(mapOf("keysJson" to """["temp",1]"""))
        }
    }
}
