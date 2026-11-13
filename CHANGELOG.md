# Changelog

All notable changes to the Offline Protocol SDK are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog covers
everything after the **v0.7.1** release.

This file holds unreleased changes and the current release. Older releases are
archived by series under [docs/changelog/](docs/changelog/); see the
[archive index](docs/changelog/README.md).

## [Unreleased]

### Added

- **Native iOS and Android applications get the SDK's built-in storage.**
  `OfflineProtocolReferenceStorage` opens the Keychain or Keystore MLS store and
  the app-container protocol-state store for one `(appId, profile)` account,
  hands both to `initializeMls`, and returns what happened to a store written
  before storage was split per account: a `warning` when this account starts
  from a fresh identity. `wipe` erases one account on sign-out. React Native
  opens and wipes its storage through the same code, so the two cannot drift.
  The iOS and Android integration guides use it in place of providers an
  application had to write itself.

### Fixed

- **Flutter on Android now reports a lost MLS identity.** Its engine host
  opened the same stores as React Native but never read the legacy-adoption
  outcome, so an account that started from a fresh identity (the earlier store
  belonged to another account, or this account's claim on it could not be
  recorded) did so with nothing logged. It now logs the same error to logcat
  that Flutter on iOS already logs to the unified log.

## [0.33.0] — 2026-11-11

> **Messages and their receipts cross a mesh of phones.** A resend now
> travels as far as the first copy did, a delivery is answered once and not
> once per path, a neighbor's receipts have a relay allowance its messages
> cannot spend, and a device's own frames go to every link a phone holds. In a
> simulated crowd of forty phones every delivery receipt now comes back, and
> on a two-hop chain of phones the receipts slower than five seconds fell from
> 118 of 200 to 15.
>
> **React Native requests the mesh's Android grants in one call.**
> `requestMeshPermissions` asks, before `start()`, for what Bluetooth and
> Wi-Fi Direct need on the Android version it runs on, and prompts only for
> what is missing.
>
> **A development build says when the integration is wrong.** React Native
> warns when a recipient is not an `off1…` address and when calls come in the
> wrong order, and a host built on `ProtocolEngineHost`, the Flutter plugin
> among them, gets the ordering hints through its diagnostic callback.
>
> **Nothing breaks a build.** No binding loses or changes an API. What behaves
> differently is in
> [§31](docs/UPGRADING.md#31-behaviour-that-changes-without-a-compile-error-v0330).

### Added

- **React Native can request the mesh's Android runtime grants in one call.**
  `requestMeshPermissions(options?)`, `requestMeshPermissionsForConfig(config)`
  and `meshPermissionOptionsFromConfig(config)` are exported from
  `@offline-protocol/mesh-sdk`. They request, before `start()`, the Bluetooth
  grants (or fine location on Android 11 and lower) and, with Wi-Fi Direct on,
  `NEARBY_WIFI_DEVICES` (or fine location on Android 12 and lower), prompting
  only for what is not already granted. On iOS they return at once: the system
  prompts when the transports start. `ACCESS_LOCAL_NETWORK` is requested only
  when the caller passes `localNetwork: true`, since only an app that targets
  API 37 and declares it needs it, and JavaScript cannot read the target. The
  permission is named by its string, so the helpers work on React Native
  releases before 0.87, which lack the constant
  ([mobile permissions](docs/mobile-permissions.md#react-native-helper-optional)).

- **React Native warns in development when a peer id is not an address.**
  `sendMessage`, the invite, MLS, service, attachment and block calls log one
  console warning per method and id when the id they are given is not a
  44-character `off1…` address, which is what a username, a `profile` or a
  phone number passed as a recipient looks like. The call still goes through.
  `devPeerAddressWarnings` on `ProtocolConfig` turns it off, or on outside a
  development build; the warning prints the id, so do not ship it on
  ([identity for integrators](docs/identity-for-integrators.md)).

- **React Native warns in development when calls come in the wrong order.**
  `sendMessage` or `enableTransport` before `await start()`, a `DataStore`
  call other than `setInterest` before the store opens, `start()` with no
  listener registered, a send with encryption on and MLS not initialized,
  and a
  `setInterest` after `start()` that narrows a space's interest each log one
  console warning and never block the call. `devLifecycleWarnings` on
  `ProtocolConfig` turns them off, or on outside a development build
  ([integrator map](docs/integrator-map.md#react-native-lifecycle-ordering)).

- **A host built on `ProtocolEngineHost` gets the same ordering hints.** The
  Flutter plugin, and a native host written against the shared bridge
  sources, receive them through `onDiagnostic` at level `warning`: a send or
  `enableTransport` before `start()`, `start()` or a send with encryption on
  and MLS not initialized, a data call the engine refuses for want of
  storage, and a `setInterest` after `start()` that narrows the interest. On
  iOS they follow `DEBUG` in the application's build. On Android the host
  takes `integratorLifecycleWarnings`, off by default, which the Flutter
  plugin sets from the application's `FLAG_DEBUGGABLE`
  ([integrator map](docs/integrator-map.md#react-native-lifecycle-ordering)).

### Changed

- **The shipped libraries build on `rustix` 1.1.6.** RUSTSEC-2026-0486, an
  unsoundness notice (not a vulnerability), reports that the
  extended-attribute functions of `rustix` 1.0.0 to 1.1.5 mark unwritten
  bytes as initialized when handed a zero-length buffer. 0.32.0 was built on
  1.1.5. The SDK calls none of those functions: it uses `rustix` only to lock
  the file store's record file, so no released library was affected. The
  dependency audit now runs on every pull request and once a day, where it
  used to run only when a dependency file changed.

### Fixed

- **A delivery receipt no longer re-sends every message still waiting for one,
  unless the peer is parked.** When a receipt settled a message carried across
  the mesh, the engine re-sent all the peer's other waiting messages at once.
  Every message of a burst is waiting, so ten messages cost 45 more frames on a
  neighbor that accepts fifteen at once. Its refusals lost receipts, which are
  never resent, and delayed messages, and each late receipt started another
  wave. A receipt now re-sends only for a peer parked as unreachable, as the
  other settlement path already required, and that re-drive is still unbounded
  when it fails or the relay parks it again. For anyone else it schedules the
  peer's other waiting messages one acknowledgement timeout away, spaced at half
  the rate a neighbor accepts, so a message that backed off while the route was
  gone doesn't wait out the rest of a wait of up to five minutes, and one
  resting with `edge_driven_unreachable_dm` still goes when a receipt shows its
  recipient is back. On a two-hop chain of phones, over ten bursts of ten
  messages each way, the first device's frames for about 107 messages went from
  846 to 308, receipts over five seconds from 118 of 200 to 68, the slowest from
  34 s to 18 s, and refused frames from 189 to 13. Those runs predate the
  scheduling, which the simulator covers.

- **A resend no longer queues behind its own first copy.** A frame whose
  earlier copy is still waiting in a local carrier queue has not gone out, so
  nothing can have answered it, and the retry ladder resent it anyway. On a
  phone chain with a slow Bluetooth link, its 1 s and 3 s resends queued
  behind the first copies, and the receipts for the sender's other messages
  waited behind them: ten copies ahead of the receipt that completed a round
  in a fast round, twenty in a slow one. The ladder now holds such a resend
  for one more rung, and only within a minute of the last copy handed to a
  carrier, directly or to a neighbor, so a link that died without telling the
  transport still cannot pin a message to it. A held resend spends no place
  in the ladder's batch and no resend token.

- **The browser hands its Bluetooth link a few fragments at a time.** The
  worker passed every fragment to the page at once, into one write queue per
  link, so the engine saw nothing queued: it could neither put a receipt
  ahead of data nor hold a resend behind its first copy. It now keeps at most
  four unwritten fragments per link with the page, and a Bluetooth sink's
  `write` may return a promise that settles when the write does, which paces
  the node. A sink that returns nothing paces nothing, as before. A write
  still pending after ten seconds closes its link (`writeTimeoutMs` on
  `BluetoothCarrier`), since one that never settled held every link's
  output. In one run
  of ten rounds on the phone chain, with the fix above and #706, receipts
  over 5 s fell from 68 of 200 to 15, each of the 15 a receipt the next hop
  refused under its rate limit.

- **A neighbor's delivery receipts are no longer refused for its messages'
  rate.** A relay admits a limited number of frames from each neighbor, and
  receipts shared that allowance with data, so a sender's own messages and
  resends could spend it and the next hop then refused the receipt frame
  behind them. Every receipt it carried waited for the sender's retry: on a
  phone chain, one refused receipt frame in each of three rounds out of ten
  left five receipts 15 s late. A link's acknowledgements (frames carrying
  `ack_for`) now have an allowance of their own, half of `peerBurst` and
  `peerRatePerSec`, that its messages cannot spend, and spend the ordinary
  one once theirs is gone, so none is refused that sharing one would have
  admitted. Nothing authenticates the marker, so a neighbor that marks
  frames gains half an allowance more admissions and no more; a marked
  frame is consumed as an acknowledgement at its recipient, and the
  device's forwarding budget still caps what goes on the air.

- **A device hands its own frame to every neighbor a phone can hold.** A
  forward's fan-out of three covers every link of a four-link phone but the
  one the frame came from, and a device's own frame was given the same
  three, so one of its four links was left out, chosen by the message id. A
  message whose only way on was that link never left, nor did any resend of
  it, and a delivery receipt was lost the same way when the link left out
  was the one the message arrived on. A device's own frames now go to one
  more neighbor than a forward.

- **A delivery carried across the mesh is answered once, not once per path.**
  A recipient answered every copy it heard, and one delivery arrives by as
  many paths as neighbors carried it, so each message cost up to four answer
  floods in a crowd of phones holding four links each. A copy a relay carried
  is no longer answered when the same message, under the same resend
  generation, was answered in the last five seconds. A copy from the sender
  itself, one over any other carrier, and a resend under a new generation are
  still always answered. In a simulated crowd of forty phones this cut the receipt frames
  by a third to two thirds and the total airtime by up to a half.

- **A resend crosses the mesh as far as the first copy did.** Every device
  that carries a frame ignores its id for ten minutes, so a sender's resend
  stopped at the first forwarder: the device beside the sender carried it
  again, but to every device after it the copy was an echo. A message lost
  past the first forwarder could not be recovered by retrying, and neither
  could a lost delivery receipt, since the recipient answers only a copy
  that reaches it. A resend that reaches a neighbor a full acknowledgement
  timeout or more after the last generation did now moves to the next generation
  (at most three, and time out of range does not count), carried in a new reserved
  metadata key, `__resend`, and a forwarder suppresses each generation
  separately, so it is a new flood whose own copies are still suppressed.
  Earlier resends stay copies of the first flood, so a receipt that is merely
  slow costs nothing extra; a message stays at the last generation for as long
  as it lives, so one for a recipient who is gone crosses the mesh at most once
  per ten minutes from then on, as it did before. In a simulated crowd of forty
  phones, every delivery receipt now comes back; at one hand-off in ten lost, every message
  arrived and more than nine in ten were acknowledged within 35 seconds, where
  a message or receipt lost past the first forwarder used to stay lost. Older
  senders send no stamp and behave as before.

- **A forward that runs out of send budget partway through its fan-out is
  finished when the budget refills.** The device counted it forwarded at the
  cut, so the frame went to whichever neighbor came first, chosen by a hash
  rather than by where its recipient is, and its id stayed suppressed for ten
  minutes. In a simulated crowd of forty phones with six conversations at
  once, the delivery receipts outran the budgets this way and 4 to 7 of about
  20 never came back; this fix alone brought that to 0 to 2, and the resend fix
  above the rest. The remaining neighbors
  now get the frame within the few seconds a queued forward is kept, and
  only those chosen in the first place, so the fan-out is never widened.

- **A mesh-only device stops flooding its neighbors with probes for peers that
  are gone.** A pending 1:1 session was probed every five seconds forever when
  no relay was there to say its peer was unreachable, and pending sessions are
  restored at every start, so a phone that had met a few devices (or earlier
  installs of the same app) kept probing all of them. Each probe was offered to
  every neighbor on every resend: on two phones over Bluetooth, eight such
  peers held the only link for minutes and a service subscription's reply
  waited behind them. A probe that only the mesh could carry now backs off on
  the relay verdict's 15 s to 600 s ladder, and discovering the peer probes it
  again at once.

- **A Python service with phones on its Bluetooth peripheral reaches the
  engines behind its peer stream.** The peripheral announced each subscribed
  central under its Bluetooth id as well as under the address its hello
  proved, so every phone was two Bluetooth peers in the core. The mesh fan-out
  ranked links by quality alone, and a peer stream, which measures no signal,
  scores mid-scale: with two phones linked, the four Bluetooth links took all
  three slots on every send, and a message for an engine two hops away was
  never delivered, on any retry. The peripheral now announces a central only
  under its proven address, and the fan-out gives the strongest link on each
  carrier a slot before any carrier gets a second, so three phones and one
  peer stream no longer starve the stream either.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.32.x](docs/changelog/0.32.md) | 0.32.0 |
| [0.31.x](docs/changelog/0.31.md) | 0.31.0 |
| [0.30.x](docs/changelog/0.30.md) | 0.30.0 |
| [0.29.x](docs/changelog/0.29.md) | 0.29.0 |
| [0.28.x](docs/changelog/0.28.md) | 0.28.0 |
| [0.27.x](docs/changelog/0.27.md) | 0.27.0 |
| [0.26.x](docs/changelog/0.26.md) | 0.26.0 |
| [0.25.x](docs/changelog/0.25.md) | 0.25.0 |
| [0.24.x](docs/changelog/0.24.md) | 0.24.1, 0.24.0 |
| [0.23.x](docs/changelog/0.23.md) | 0.23.0 |
| [0.22.x](docs/changelog/0.22.md) | 0.22.0 |
| [0.21.x](docs/changelog/0.21.md) | 0.21.0 |
| [0.20.x](docs/changelog/0.20.md) | 0.20.1, 0.20.0 |
| [0.19.x](docs/changelog/0.19.md) | 0.19.0 |
| [0.18.x](docs/changelog/0.18.md) | 0.18.3, 0.18.2, 0.18.1, 0.18.0 |
| [0.17.x](docs/changelog/0.17.md) | 0.17.0 |
| [0.16.x](docs/changelog/0.16.md) | 0.16.6, 0.16.5, 0.16.4, 0.16.3, 0.16.2, 0.16.1, 0.16.0 |
| [0.15.x](docs/changelog/0.15.md) | 0.15.0 |
| [0.14.x](docs/changelog/0.14.md) | 0.14.0 |
| [0.13.x](docs/changelog/0.13.md) | 0.13.1, 0.13.0 |
| [0.12.x](docs/changelog/0.12.md) | 0.12.0 |
| [0.11.x](docs/changelog/0.11.md) | 0.11.1, 0.11.0 |
| [0.10.x](docs/changelog/0.10.md) | 0.10.0 |
| [0.9.x](docs/changelog/0.9.md) | 0.9.4, 0.9.3, 0.9.2, 0.9.1, 0.9.0 |
| [0.8.x](docs/changelog/0.8.md) | 0.8.0 |
