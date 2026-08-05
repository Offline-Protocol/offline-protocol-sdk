package com.offlineprotocol

/**
 * Runs a teardown to completion even when one of its steps throws, then hands
 * the caller the failures it collected.
 *
 * A teardown here stops several independent things — the process scheduler,
 * five transports, the keep-alive service, the protocol core. Written as a
 * plain statement list, the first throw skips every step after it: one
 * transport that fails to stop leaves the rest running, while the paths that
 * *report* the teardown (the notification coming down, `mesh_stopped_by_user`
 * reaching JS) still say the mesh is off. That is not hypothetical — a BLE stop
 * runs `stopScan` on an adapter the user may have just turned off, and the
 * resulting `IllegalStateException` crosses back to the caller. The steps do
 * not depend on each other, so a failure in one must not decide whether the
 * others run.
 *
 * Failures are collected rather than reported inline: a reporter that throws
 * would abort the very sequence it is describing, and a caller usually wants to
 * report only once everything is actually down. [firstFailureOrNull] is what a
 * caller rethrows to keep its own error contract — it is the exception that
 * would have propagated before, so a `stop()` still rejects with the same
 * cause, just after the remaining steps have run.
 *
 * Only [Exception] is caught. An [Error] is not "a step failed", it means the
 * process is already in trouble, so it propagates immediately and abandons the
 * rest — which is what it should do.
 *
 * Not thread-safe: one sequence belongs to one teardown pass on one thread.
 */
class TeardownSequence {

    /** A step that threw, named as the caller named it. */
    data class Failure(val step: String, val cause: Exception)

    private val collected = mutableListOf<Failure>()

    /** The failures so far, in the order they happened. */
    val failures: List<Failure> get() = collected

    /**
     * Runs [action], recording a throw against [step] instead of propagating
     * it, so the steps that follow still run.
     */
    fun step(step: String, action: () -> Unit) {
        try {
            action()
        } catch (e: Exception) {
            collected.add(Failure(step, e))
        }
    }

    /**
     * The first failure's cause, for callers that must still surface one — a
     * `stop()` rejecting its promise. Null when every step ran clean.
     */
    fun firstFailureOrNull(): Exception? = collected.firstOrNull()?.cause
}
