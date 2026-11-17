# 0038. Two devices that meet over Bluetooth bring up Wi-Fi through their session

**Status:** Accepted

## Context

An iPhone and an Android phone with no shared network meet only over
Bluetooth LE ([ADR 0028](0028-android-peer-streams-join-the-lan.md): "With no
shared network they still do not meet"). Every attachment between them is
capped by Bluetooth: a 1 MiB file took 197.7 s on the lab pair, about
5.3 KB/s, and an envelope over about 71 KB does not fit at all.

The engine already has a slot for a faster link, the peer stream
([stream framing](../spec/stream-framing.md)). Once a stream's preamble proves
the peer's address, everything above the slot works: the transport selector
weights the slot by bandwidth, a file transfer pins to it once a stream proves
the recipient, and outboxes and acknowledgements make falling back to
Bluetooth lossless for messages. A file transfer keeps the carrier it started
on, so one in flight when a link ends fails at once
([ending a link](../state-machines/fast-link.md#ending-a-link)) and the
application sends it again. What two phones without a shared network lack is
a network to open the stream over.

Neither platform lets an application make one silently, but each lets it make
one with credentials:

- Android 10 and later creates a Wi-Fi Direct group with a chosen network name
  and passphrase (`WifiP2pConfig.Builder`), and its owner is an ordinary WPA2
  access point to a device that does not speak Wi-Fi Direct. The SDK already
  forms groups this way, with credentials derived from the application id
  ([R22](../security/threat-model.md#r22-an-android-wi-fi-direct-group-admits-anyone-who-knows-the-application-id)).
- iOS joins a network by name and passphrase with `NEHotspotConfiguration`,
  after a system prompt. It cannot host one.

A spike on a Samsung SM-M176B and an iPhone 16 (November 2026) settled the
open question: the iPhone joined the Samsung's group from credentials it was
handed, dialed the Samsung's existing listener and the preamble verified in
both directions. Join tap to verified preamble took 11.3 to 13.6 s, about
10 s of it inside iOS between its scan and its association. The 1 MiB file took
8.0 to 8.7 s. The Samsung stayed on its own Wi-Fi throughout. The iPhone
showed "“App” Wants to Join Wi‑Fi Network “NAME”?" on every join, left its own
Wi-Fi for the group, and, with `joinOnce`, disconnected 15 s after the app left
the foreground and deleted the passphrase.

## Decision

1. **A fast link is a Wi-Fi network one device hosts for one peer, whose
   credentials travel inside their confirmed 1:1 MLS session.** The stream
   opened over it is an ordinary peer stream: the same preamble, the same
   framing, the same slot. The engine above the slot learns nothing new about
   it.

2. **It is brought up on demand, never on discovery.** The application calls
   `request_fast_link(peer)`, and `close_fast_link(peer)` ends it early. There
   is no automatic trigger. Every join on iOS puts a system prompt in front of
   the user and takes the phone off the user's own network, so whether that is
   worth it for a given transfer is the application's call, not the engine's.

3. **The platform says what it can do, and the engine assigns the roles.** A
   platform declares `can_host` and `can_join` once it knows its permissions:
   Android 10 and later both, iOS join only, everything else neither. A
   requester that can host hosts; one that can only join asks its peer to
   host. When both ask at once, the lower address (compared by UTF-8 bytes, as
   for streams) hosts and the other side joins. Whether a device takes part in
   a link its peer starts is its application's choice too, through
   `wifiDirect.fastLink`: `any` (the default) hosts for a peer's request and
   joins an offer it did not ask for, `requestedOnly` answers both `declined`,
   `off` declares no role.

4. **The engine makes the credentials, fresh for every link.** The network
   name is `DIRECT-`, two random characters, `-` and twelve random hex digits:
   Android requires the prefix, and nothing in the name derives from the
   application, the user or the device ([ADR 0037](0037-an-applications-users-are-visible-only-to-that-application.md)).
   The passphrase is 32 random hex digits, 128 bits. The host's platform
   brings the network up with exactly these and reports the address and port
   its listener holds.

5. **Three frames, under one new prefix, `__FAST_LINK__`, which only ever
   travels inside a decrypted 1:1 MLS plaintext**, like `__DATA_V1__`:
   `request`, `offer` and `close`. Each is sealed with the strict encryptor (a
   confirmed session or nothing) and sent once over the live direct link with
   no acknowledgement, outbox or retry ([ADR 0032](0032-ephemeral-sends-go-once-over-a-live-link.md)).
   An offer delivered late is worse than one lost: the network it names is
   gone, and the joiner would prompt its user for nothing. A frame is sent
   only to a peer whose key package advertised `link_versions` entry 1.
   MLS authenticates the sender, hides the passphrase, and refuses a replayed
   ciphertext; the offer's own lifetime bounds a slow delivery.

6. **The stream proves it reached the right peer, and the host admits only
   the invitee.** The joiner dials the offered address and port with the
   host's address as its claim, so a different preamble closes the stream
   (step four of the preamble). The host refuses, on the network it made, any
   proved address other than the one it invited. The offered address must be
   a private or link-local IP literal: never a name to resolve, never a route
   off the network just joined. An iPhone dials it bound to Wi-Fi, having
   left its own network for the group. An Android joiner stays on its own
   Wi-Fi while in a group, so an offered private address could name a host on
   the user's home network: it dials the group owner's address as the group
   reports it, with the offer's port, and never the offer's host.

7. **The engine ends a link** when its offer expires unused (120 s), when the
   proved stream is lost, after 60 s with nothing large carried (a body of
   16 KiB or more) and no transfer to the peer under way, on either side's `close`, and on the application's
   `close_fast_link`. A joiner's platform reports its own end when it can no
   longer hold the network: iOS does when the application leaves the
   foreground, because iOS disconnects 15 s later anyway and the host would
   otherwise hold a dead stream until keepalive notices, measured at 40 s.
   Bluetooth stays up throughout and carries the traffic before, during the
   switch and after.

8. **One link at a time per device.** A phone joins one Wi-Fi network at a
   time and an Android device owns at most one Wi-Fi Direct group, so a second
   request is refused as busy rather than queued.

9. **The peer-stream slot keeps 512 KiB in flight: eight 64 KiB chunks, not
   four of 32 KiB.** With four of 32 KiB the transfer waited on
   acknowledgements: about 128 KiB a round, which is what the spike measured
   (126 KB/s), far below the link. The bytes grow through the chunk size and
   not the chunk count because each chunk is a ratchet generation, and the
   receiver keeps 32 for late arrivals: two transfers of sixteen would use
   them all. Every other slot keeps its window.

### Why not LocalOnlyHotspot as a fallback

Android's `LocalOnlyHotspot` worked in the spike, but the system picks its
credentials: a passphrase of 15 lowercase letters and digits, about 77 bits,
under the floor of decision 4, which an application cannot raise. It also
needs location on older releases, and on phones without concurrent station
and access point it drops the user's own Wi-Fi. No case was seen where Wi-Fi
Direct failed and it would have helped, so it is left out rather than shipped
untested.

### Why not derive the credentials, as group formation does

That is R22: a passphrase anyone with the application id can compute admits
anyone. Formation derives them because two devices that have not heard each
other have no channel to agree on a secret. A fast link always has one, the
session, and only uses it.

## Consequences

- An iPhone and an Android phone with no shared network move a 1 MiB file in
  about 2.5 s instead of 197.7 s over Bluetooth, after one prompt on the
  iPhone (8 s before decision 9). A message
  too large for Bluetooth fits the slot's 1 MiB ceiling.
- The join takes the iPhone off its own Wi-Fi for as long as the link lasts,
  and the link lasts only while the application is in the foreground. An
  application should ask for a link when a transfer is worth that, and expect
  it to end when the user leaves.
- An iOS application that uses it needs the Hotspot Configuration entitlement
  (`com.apple.developer.networking.HotspotConfiguration`) and local-network
  permission. An Android application needs `NEARBY_WIFI_DEVICES` on Android 13
  and later, and fine location below it.
- A fast link and Wi-Fi Direct group formation (`wifiDirect.autoAccept`) share
  the one group an Android device can own: while either holds it, the other
  is refused.
- The network's name and existence are visible to anyone in radio range for
  as long as it is up, and its passphrase only to the two devices
  ([R34](../security/threat-model.md#r34-a-fast-link-network-is-visible-in-range-and-its-offer-trusts-the-session)).
- The runtime behaviour is the [fast link state machine](../state-machines/fast-link.md).

## What would undo this

Wi-Fi Aware on both platforms would let two phones open a stream without
either one hosting a network or prompting, and the slot would not notice the
change. Until a pairing-capable Android phone proves it against an iPhone,
this is the path. Going back to deriving credentials would reopen R22 for a
case that has a secure channel to use.
