package com.offlineprotocol

import android.content.Context
import uniffi.offline_protocol.MlsStorageProvider
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolStateStorageProvider

/**
 * G7: shipped reference MLS + protocol-state backends for native Android integrators.
 *
 * Same stores the React Native bridge wires in [ProtocolEngineHost.initializeMlsWithSecureStorage].
 */
object OfflineProtocolReferenceStorage {
    /** Stable namespace for one `(appId, profile)` pair; matches built-in providers. */
    fun accountNamespace(appId: String, profile: String): String =
        StorageNamespace.account(appId, profile)

    fun makeSecureStorage(
        context: Context,
        appId: String,
        profile: String,
    ): MlsStorageProvider {
        val namespace = StorageNamespace.account(appId, profile)
        return MlsSecureStorage(context, namespace)
    }

    fun makeProtocolStateStorage(
        context: Context,
        appId: String,
        profile: String,
    ): ProtocolStateStorageProvider {
        val namespace = StorageNamespace.account(appId, profile)
        return AppContainerProtocolStateStorage(context, namespace)
    }

    /** Opens both built-in stores for `appId` / `profile` and calls [OfflineProtocol.initializeMls]. */
    fun initializeMls(
        protocol: OfflineProtocol,
        context: Context,
        appId: String,
        profile: String,
    ) {
        protocol.initializeMls(
            makeSecureStorage(context, appId, profile),
            makeProtocolStateStorage(context, appId, profile),
        )
    }
}
