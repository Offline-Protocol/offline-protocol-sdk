package com.offlineprotocol

import android.content.Context
import uniffi.offline_protocol.MlsStorageProvider
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolStateStorageProvider

/**
 * Opens, and erases, the SDK's built-in storage for one `(appId, profile)`
 * account: the `EncryptedSharedPreferences` MLS store and the app-container
 * protocol-state store.
 *
 * This is the one place those two stores are wired together. The React Native
 * module and the engine host both call it, so a native application that calls
 * it gets the same stores, the same account namespace and the same legacy
 * adoption a React Native application gets. A second copy of the wiring is how
 * the two drift: a host that derived the namespace differently would open an
 * empty store and start a fresh identity with no error.
 */
object OfflineProtocolReferenceStorage {

    /**
     * What happened to the store written before storage was split per account,
     * the first time this account's secure store was opened.
     *
     * [CONFLICT] and [CLAIM_UNVERIFIED] mean this account starts from a fresh
     * MLS identity. They must reach someone: [warning] is the text the React
     * Native module reports for them.
     */
    enum class LegacyAdoption {
        /** There was no earlier store to inherit. */
        NONE,

        /** This account inherited the earlier store on this launch. */
        ADOPTED,

        /** This account inherited the earlier store on a previous launch. */
        RESUMED,

        /**
         * The earlier store belongs to another account. This account starts
         * from a fresh identity, an empty outbox and an empty block list.
         */
        CONFLICT,

        /**
         * This account's claim on the earlier store could not be recorded, so
         * it was not inherited. This account starts from a fresh identity, an
         * empty outbox and an empty block list.
         */
        CLAIM_UNVERIFIED;

        /** Whether this account starts from a fresh MLS identity although an earlier one exists. */
        val startsFreshIdentity: Boolean
            get() = this == CONFLICT || this == CLAIM_UNVERIFIED

        /** The error to report when [startsFreshIdentity], otherwise null. */
        val warning: String?
            get() = when (this) {
                CONFLICT ->
                    "Legacy secure store belongs to another account; this account starts from a " +
                        "fresh MLS identity and cannot decrypt its previous sessions. Its pre-split " +
                        "delivery state is unreachable too, so it also comes up with an empty outbox " +
                        "and an empty block list: every previously blocked peer is unblocked"
                CLAIM_UNVERIFIED ->
                    "Could not record this account's claim on the legacy secure store, so it was " +
                        "not adopted: another account could otherwise inherit the same MLS identity. " +
                        "This account starts from a fresh identity and comes up with an empty outbox " +
                        "and an empty block list: every previously blocked peer is unblocked. The " +
                        "credential store is failing writes; retrying on a healthy store adopts normally"
                NONE, ADOPTED, RESUMED -> null
            }

        internal companion object {
            fun of(decision: LegacyStoreAdoption.Decision): LegacyAdoption = when (decision) {
                is LegacyStoreAdoption.Decision.Adopt -> ADOPTED
                is LegacyStoreAdoption.Decision.Resume -> RESUMED
                is LegacyStoreAdoption.Decision.Conflict -> CONFLICT
                is LegacyStoreAdoption.Decision.ClaimUnverified -> CLAIM_UNVERIFIED
                is LegacyStoreAdoption.Decision.None -> NONE
            }
        }
    }

    /**
     * The `EncryptedSharedPreferences` MLS store for one account.
     *
     * This does not report what happened to an earlier store on the device, so
     * a lost identity passes silently. Use [initializeMls] unless you need the
     * store on its own.
     */
    @JvmStatic
    fun makeSecureStorage(context: Context, appId: String, profile: String): MlsStorageProvider =
        MlsSecureStorage(context, StorageNamespace.account(appId, profile))

    /** The app-container protocol-state store for one account. */
    @JvmStatic
    fun makeProtocolStateStorage(
        context: Context,
        appId: String,
        profile: String,
    ): ProtocolStateStorageProvider =
        AppContainerProtocolStateStorage(context, StorageNamespace.account(appId, profile))

    /**
     * Opens both stores for one account and hands them to
     * [OfflineProtocol.initializeMls].
     *
     * Call it after `setEventCallback` and before `start()`, with the `appId`
     * and `profile` the engine was configured with. Report the result's
     * [LegacyAdoption.warning] when it has one: it is the only signal that this
     * account lost its earlier identity.
     */
    @JvmStatic
    fun initializeMls(
        protocol: OfflineProtocol,
        context: Context,
        appId: String,
        profile: String,
    ): LegacyAdoption {
        val namespace = StorageNamespace.account(appId, profile)
        val secure = MlsSecureStorage(context, namespace)
        val state = AppContainerProtocolStateStorage(context, namespace)
        protocol.initializeMls(secure, state)
        return LegacyAdoption.of(secure.legacyAdoption)
    }

    /**
     * Erases everything the built-in stores hold for one account: its MLS
     * identity, sessions and pins, its outbox and its protocol state.
     *
     * Stop the engine for this account and release it first. A running engine
     * keeps writing, and releasing an engine flushes its state, so an engine
     * released after this call writes into the stores it just emptied.
     *
     * The secure store goes first: it holds the key every sealed protocol-state
     * record is written under, so an interrupted wipe leaves the rest as
     * ciphertext nobody can open rather than readable state. Both are attempted
     * even if the first fails, and the first error is rethrown. Idempotent:
     * call again after an error.
     */
    @JvmStatic
    fun wipe(context: Context, appId: String, profile: String) {
        val namespace = StorageNamespace.account(appId, profile)
        var firstError: Exception? = null
        try {
            MlsSecureStorage.wipeAccount(context, namespace)
        } catch (e: Exception) {
            firstError = e
        }
        try {
            AppContainerProtocolStateStorage.wipeAccount(context, namespace)
        } catch (e: Exception) {
            if (firstError == null) firstError = e
        }
        firstError?.let { throw it }
    }
}
