# 0039. A browser dials a native device on its network from a code the device shows

**Status:** Proposed

## Context

A page that runs the browser node links to other browsers over WebRTC
([web bridge W9](../bridges/web.md#w9-webrtc-lives-on-the-page-and-is-set-up-offline)),
to a phone or a board over Bluetooth LE in Chromium (W14), and to anything
over Nostr when there is internet (W12). It cannot link to the native app on
the same Wi-Fi network. The phone's peer-stream listener is a TCP socket
([stream framing](../spec/stream-framing.md)), and a page cannot open one.
So on a network with no internet, a page and a phone side by side share
nothing faster than Bluetooth, and nothing at all in Safari or Firefox,
which have no Web Bluetooth.

A page can open one kind of connection to a device it has never met:
WebRTC. It normally needs the far side's session description, which is why
W9 has two codes that people carry by hand. libp2p's WebRTC Direct showed a
way around that. The listener is an ICE-lite responder with a self-signed
DTLS certificate, and the dialler builds the listener's description itself
from an address, a port and the certificate's hash. Its first version also
edits the browser's own local description, to set an ICE username and
password the listener can predict. libwebrtc is removing that edit (the
`WebRTC-NoSdpMangleUfrag` field trial), and its SDP-munging plan names
libp2p as a user.

A spike in November 2026 tested a variant that edits nothing. The listener
picks its own ICE username and password and hands them out with the
certificate hash. The page builds only the remote description, and the
listener checks every STUN request's integrity with its own password. The
spike used a str0m 0.25 listener on a Mac and a page in Playwright.

- **Every engine linked:** Chromium 156, Firefox 157 and WebKit 27.2 all
  connected with default settings, mDNS host candidates included. The
  listener learns the browser's address from the source of its first check
  (a peer-reflexive candidate), so it never needs the browser's candidates.
- **Engines agreed on the refusals:** a wrong certificate hash failed at DTLS
  in all three, and a wrong password failed at every check.
- **Concurrent dials were kept apart:** three pages dialling at once on one
  code were told apart by their own ICE usernames.
- **The no-edit variant survives the field trial:** with
  `WebRTC-NoSdpMangleUfrag` enabled, Chromium refused a v1-style edit ("SDP
  is modified in a non-acceptable way") and accepted this variant.
- **Real iPhone Safari linked:** on an iPhone 16 (iOS 27.2) on one Wi-Fi
  network, Safari linked 10 times in 10, from an HTTPS page and from a plain
  HTTP one. One MiB took 0.2 to 1.6 s each way.

The real Safari runs also found a delay. Connect took 82 to 250 ms or 1.2 to
3.0 s. A per-datagram trace showed why. str0m's ICE-lite agent sends nothing
until a check carries USE-CANDIDATE, so it held the browser's DTLS hello
until Safari nominated a pair, which Safari sometimes did about one second
after its first check.

## Decision

1. **The native app can listen for browsers, and its stream is an ordinary
   peer stream.** A WebRTC data channel from a page that dialled the device
   fills the peer-stream slot the FFI names `wifi_direct`. It uses the same
   preamble, the same framing and the same caps. The engine above the slot
   learns nothing new. A DTLS certificate is never an identity: the preamble
   proves the address, and the preamble only.

2. **The dial code is the whole setup.** The listener's code names a private
   IP address, a UDP port, the SHA-256 hash of its certificate, its ICE
   username and password, and its Offline Protocol address. It is a URL
   fragment, `#op-direct=1&...`
   ([stream framing](../spec/stream-framing.md#dialling-a-listener-from-a-browser)),
   which an application shows as a QR code or text. A fragment never reaches
   a web server, so a code opened in a browser does not put the network
   address and password in a server's log. No server, relay or second code
   is involved.

3. **The page builds only the remote description, and never edits its
   own.** Its local description is the one the browser generated. The
   listener's username and password come from the code, and the browser's
   come from the browser. This is what keeps the dial working after
   libwebrtc stops accepting edits to a local description, which is how
   libp2p's v1 dials work today.

4. **A datagram makes the listener keep state only if it is an
   authenticated check.** It must be a STUN binding request whose USERNAME
   starts with the listener's username and whose MESSAGE-INTEGRITY verifies
   under the listener's password. Anything else is dropped, with no reply
   and no allocation. Without this, every device on the network could make
   the listener hold a DTLS and SCTP stack per packet it sends. A reply to
   an unauthenticated packet would also make the phone a reflector.

5. **The listener answers on the first authenticated pair, without waiting
   for a nomination.** An ICE-lite agent cannot run checks of its own, so a
   check that passed integrity is all the validation it will ever have. str0m
   0.25 waits for USE-CANDIDATE, which cost real Safari about one second on
   some dials. A later nomination still moves the pair. The change is a few
   lines in str0m's ICE agent, and is offered upstream. Until a release
   carries it, the listener marks the first authenticated request of a dial
   as nominating before str0m reads it.

6. **Of two dialled streams for one address, the listener keeps the newer.**
   Only the page dials, so the race the lower-address rule settles (both
   ends dialling at once) cannot happen here. A page that reloads, or comes
   back after a Wi-Fi drop, would otherwise be refused for up to 15 s while
   the listener held the dead stream. Against a stream of another kind
   proving the same address, the existing stream stays. On its side, the
   page keeps its first link as before (W3), so before dialling a code
   again, it closes its earlier connection to that address. If the page kept
   its old link while the listener kept the new one, each end would close
   the link the other kept.

7. **Limits apply before an address is proved.** At most four dials may be
   pending (authenticated but not yet proved). A dial that has not proved an
   address within 10 s of its first check is dropped. A proved stream then
   counts against the slot's caps like any inbound stream: 16 streams, 12
   inbound and 4 from one remote address on mobile. An ICE-lite listener
   cannot send checks, so it ends a stream when the browser's consent checks
   have been silent for 15 s. Outbound frames are cut into chunks of at most
   16 KiB under a 1 MiB buffered high-water mark. A stream with more than
   8 MiB queued is closed, the bound the page already applies (W9).

8. **The listener is opened and closed by the application, and is fresh
   each time.** Opening it makes a new certificate, a new username and
   password, and a new UDP port. Closing it ends its streams. A code
   therefore admits dials for as long as the application keeps that
   listener open, and never again after. On iOS the listener lives only
   while the application is in the foreground.

9. **The work is one sans-I/O Rust crate under the bridges, and the platform
   owns the socket.** `offline-protocol-webrtc-direct` is std-only and never
   reaches core, sealed or leaf. It takes datagrams and timer ticks and
   gives back datagrams to send. It runs the framing and the preamble with
   the one `PeerStreamReader`, and hands the bridge three events: a stream
   proved an address, a body arrived, a stream ended. The bridge keeps its
   stream table and its one-stream-per-address rule, and calls
   `wifi_direct_*` as it does for TCP. The socket is the platform's so that
   it can be bound to the Wi-Fi network, as ADR 0028 does for Android dials,
   or, when the phone shares its own hotspot and is on no Wi-Fi network, to
   that hotspot's interface address, so a device that joined the hotspot
   can dial with no other network at all.

10. **WebRTC comes from str0m, with the platform's own crypto where it has
    one.** On Apple platforms str0m uses Apple's crypto; elsewhere it uses
    aws-lc-rs. The FFI crate builds it behind the feature `webrtc-direct`.
    The iOS, Android and Python packages the release ships turn the feature
    on, and so do the desktop libraries, since the wheels are built from
    them. A library built without it refuses `webrtc_direct_open`.

### Why not libp2p's WebRTC Direct as specified

The libp2p spec authenticates both ends with a Noise handshake over the data
channel and identifies peers by libp2p PeerId. This protocol already has its
own preamble for identity and MLS for content, so Noise would add a third
handshake with nothing more to prove. libp2p's v1 also depends on the edit
that libwebrtc is removing (decision 3).

### Why not webrtc-rs, libdatachannel or libwebrtc

- **webrtc-rs `rtc` 0.21:** pre-1.0, and it offers only ring or aws-lc for
  crypto, so Apple's crypto is not an option and iOS cannot save the size
  decision 10 saves.
- **libdatachannel:** C++. Calling it puts unsafe code outside the C
  boundary, which the FFI crate forbids.
- **Google's libwebrtc:** several times the size of the measured cost below,
  for media this protocol does not use.

str0m is sans-I/O, which fits a bridge that already owns its sockets.

## Consequences

- A page links to the native app on the same network from a code, with no
  internet, and in Safari and Firefox as well as Chromium. A phone scans
  the other phone's QR code. A laptop opens the URL or pastes it.
- **Size cost:** the mobile libraries grow by about 1.54 MB on iOS (Apple's
  crypto) and about 2.47 MB on Android (aws-lc-rs). Both figures are
  stripped, arm64, built with the `mobile` profile. About 1 MB of each is
  str0m, its DTLS stack and SCTP.
- **Toolchain:** str0m 0.25 needs Rust 1.89. The FFI crate already requires
  1.91. The new crate is left out of the 1.87 MSRV check, as the FFI crate
  is.
- **Where the page comes from:** the page must be one the browser already
  has, from an HTTPS origin or cached for offline use (W15). The browser
  node needs a secure context, and a page the phone served over plain HTTP
  would not be one.
- **Chrome's Local Network Access:** it does not gate WebRTC yet (November
  2026). When it does, a page from a public origin will ask before its
  first dial to a private address. If the person refuses, the browser drops
  the candidate silently and the dial times out.
- **Exposure:** the listener is a UDP port open to the network while the
  application keeps it open. It is silent to everything except
  authenticated checks. A code admits whoever holds it, for as long as its
  listener lives
  ([R35](../security/threat-model.md#r35-a-dial-code-admits-whoever-holds-it-to-an-open-listener)).

## What would undo this

- **A browser API for local discovery or raw sockets:** if one shipped in
  every engine, the page could reach the TCP listener and this listener
  could go.
- **A WebRTC change that rejects an ICE-lite answer the page built:** if
  browsers refused a remote description built this way, the dial would
  break in the same way libp2p v1 does. The Chromium field-trial run is the
  check to repeat when a browser changes how it handles SDP.
- **str0m losing maintenance:** that would put decision 10 back up for
  choice. The sans-I/O boundary in decision 9 is what keeps a replacement
  contained to one crate.
