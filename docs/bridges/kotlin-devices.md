# The Android Bluetooth LE link on two devices

The unit suite runs the central's decisions on the JVM. What it cannot show is
what two phones do when one of their apps restarts while the radios stay
linked: a Bluetooth LE link outlives the app on either side (K9), and every
failure this checklist was written for appeared only then. Run it before a
release that changes `bindings/react-native/android/.../ble`, and record the
result at the bottom of this file.

## Setup

- An Android phone and an iPhone, a metre apart, each running an application
  on the SDK with Bluetooth LE on and internet off. Any application that sends
  a message when a neighbour appears works; the runs below used flush, a poker
  table where the host sends its table to every joiner.
- The Android phone in airplane mode with Bluetooth on, so nothing crosses
  another carrier.
- Logs from both sides. Android: `adb logcat -v time`. iPhone: launch with
  `xcrun devicectl device process launch --console --terminate-existing
  --environment-variables '{"NSUnbufferedIO":"YES"}' <bundle id>`. Without
  the variable the bridge's output is block-buffered and arrives only when the
  app exits.
- Leave at least 8 seconds between Android app starts. Android refuses a sixth
  scan start within 30 seconds ("app is scanning too frequently") and the
  scan then returns nothing for 30 seconds, which looks exactly like a failed
  connect.

## The checks

Each cycle passes when the first message from the other phone arrives within
40 seconds of the restart. On a cycle that passes, the Android log has no
"takes writes and returns nothing" line: that teardown is for links that
never deliver, and a healthy connect must never trigger it.

1. **Android restarts, iPhone stays.** Force-stop and relaunch the Android
   app, ten times.
2. **iPhone restarts, Android stays linked.** Terminate and relaunch the
   iPhone app, ten times. CoreBluetooth restores the iPhone's links, so the
   Android phone sees no disconnect, and the new process holds no hello and
   no subscription from before. This is the check that failed.
3. **Both restart, Android first.** Relaunch the Android app, then the iPhone
   app about 4 seconds later, so the Android phone first links to the old
   iPhone process. Ten times.
4. **Both restart, iPhone first.** Six times.

## Results

Add a row per run: the date, the commit, both devices with their OS, and the
outcome of each check.

| Date | Commit | Devices | Notes |
|------|--------|---------|-------|
| 2026-10-29 | aa01fe5e (0.31.0) | Samsung Galaxy M17 5G (SM-M176B), Android 16, airplane mode; iPhone 16, iOS 26. flush on `@offline-protocol/mesh-sdk` 0.31.0 | 1: 15 of 15. 3: 9 of 10; the tenth linked to the old iPhone process, and once the new one took over, every send was deferred as "not yet ready (CCCD unacked)" toward the address the iPhone's hello had mapped, beside a ready client link, for the 2 minutes watched. 4: 12 of 12, the first message about 11 s after the link. 2: failed on the first restart, the same way as 3's failure |
| 2026-10-29 | aa01fe5e with K9's three changes | The same two phones. The Android app rebuilt with the changed sources; the iPhone unchanged on 0.31.0 | 1: 10 of 10, about 0.9 s from link to message. 2: 20 of 20 over two runs, the message about 8 s after the iPhone relaunch; before the duplicate-dial binding, the second restart left the iPhone's writes to our server unattributed for good. 3: 20 of 20, the message about 13 s after the Android start, 9.5 s after its first link (to the old iPhone process). 4: 12 of 12. No silent-link teardown on any of these. The teardown itself, with writes swallowed and inbound dropped in a test build until the first teardown: the link was dropped 21.5 s after it came up, redialled and ready 3.3 s later, and the first message crossed 0.9 s after that; no teardown in the 2.5 minutes after. An earlier version that judged a link again after the peer answered tore it down every 30 to 40 s in that test, which is why it no longer does |
| 2026-10-29 | 57578e23 | The same two phones. The Android app rebuilt from this commit's Bluetooth LE sources; the iPhone unchanged on 0.31.0 | 1: 10 of 10, about 1 s from link to message after the first. The first cycle came seconds after the iPhone app relaunched: the iPhone saw neither the subscription nor the hello of Android's first link, the link was dropped as silent 22 s after it was ready, and the message crossed 2.7 s after that. 2: 10 of 10, no silent-link teardown and no duplicate-dial binding. 3: 10 of 10, the message about 9.5 s after the first link. 4: 6 of 6. No silent-link teardown in 2 to 4. The teardown with writes swallowed and inbound dropped until the first one: dropped 21.5 s after the link was ready; a fresh advertisement redialled it 0.1 s later, ahead of the reconnect backoff; ready 3.4 s after the teardown, and the first message 0.7 s after that; no teardown in the 3 minutes after |
