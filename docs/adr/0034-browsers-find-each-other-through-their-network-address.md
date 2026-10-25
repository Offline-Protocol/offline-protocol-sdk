# 0034. Browsers on one network find each other through its address

**Status:** Accepted

## Context

Every native binding discovers neighbours on its own: a Bluetooth LE scan,
DNS-SD on the local network, Wi-Fi Direct service discovery. A page can do
none of these. It cannot advertise over Bluetooth, no browser exposes
multicast DNS, and Wi-Fi Direct has no web API. The Local Peer-to-Peer API
that would cover the local network is a Web Platform Incubator Community
Group draft (dated 27 May 2025) that no browser ships.

So two browsers in one room had to carry two WebRTC codes between them, or
share a room code a person reads out (W11). Nothing let them notice each
other.

What two devices on one network do share is the address the internet sees
them from: one public IPv4 address behind a home or office NAT, or one IPv6
/64. A STUN server reports it to a page as a server-reflexive ICE candidate.

## Decision

A nearby room is a Nostr topic and key derived from the application id and
that address (the IPv4 address, or the IPv6 /64). Every device in the room
publishes a presence, lists the others, and links over WebRTC to one its
person picks. The link runs on the local network; the relays carry only the
setup.

The room key is derived from something anyone can enumerate, so the design
treats it as public:

- **A presence holds only a key used for this room and the application's
  label.** Nothing identifies the device or its local addresses.
- **Every connection code is sealed to one presence key** with NIP-44, under
  the room key as a second layer. A holder of the room key reads presences
  and nothing else.
- **A host answers only what its application admits.** `admit` is required:
  a page that opens a room has said who it lets in. The label is a claim; the
  engine's preamble and MLS session are what identify the device on a link.
- **The relays and the STUN servers are the application's.** The SDK names
  neither, and a room without them does not open.

## Consequences

Same-network discovery for browsers needs the internet, to reach a relay and
a STUN server. Two devices on a network with no route out cannot use it,
and fall back to codes (W9). That is a real difference from the native
bindings, and it is permanent until browsers expose local discovery.

The room is shared with everyone behind the same address. On a carrier-grade
NAT, a hotel or a campus that can be thousands of strangers, who all see each
other's labels and can each offer. Admission is what keeps that from being a
way in, which is why it is the application's explicit decision and not a
default. An application should show the label, ask, and keep the address the
person accepts.

Devices whose traffic leaves by different routes (a phone on cellular, a
laptop on a VPN, a dual-stack network where one device reports IPv4 and the
other IPv6) land in different rooms and do not see each other. Nothing
detects that; the room is simply empty.

## What would undo this

A browser shipping local discovery (the Local Peer-to-Peer API, or an mDNS
API for pages) makes the address-derived room unnecessary on networks that
allow it, and that would be the better path where it exists. Until then,
the thing to guard against is someone treating the room key as a secret and
sealing a connection code under it alone: anyone who can guess the address
would then read the local addresses every code names.
