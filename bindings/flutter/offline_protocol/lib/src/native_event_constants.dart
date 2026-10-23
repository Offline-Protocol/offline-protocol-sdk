/// Must match `EventReplayRing.DEFAULT_CAPACITY` on Android and iOS (RN
/// `NATIVE_EVENT_REPLAY_CAPACITY`, pinned in `offline-protocol-uniffi`).
const int nativeEventReplayCapacity = 256;

/// How long dispatch waits for native `replayEvents` before continuing.
const int nativeEventReplayTimeoutMs = 2000;

/// Cap on events held while replay is in flight.
const int maxDeferredNativeEvents = 1024;
