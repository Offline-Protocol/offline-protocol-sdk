# DNS-SD mapping for service descriptors

## What this chapter is for

The mesh discovers services by a signed query that gossips hop by hop and a
signed response that walks back ([service discovery](../service-discovery.md)).
On a LAN there is a second way to learn what a neighbour offers: DNS-SD
(RFC 6763) over multicast DNS (RFC 6762), which every desktop operating
system already answers. This chapter fixes how a `ServiceDescriptor` is laid
out as a DNS-SD instance, so that two implementations on one LAN publish and
read the same records, and it fixes what an implementation may and may not do
with a record it did not sign.

The mapping is optional in both directions. An implementation that never
publishes and never browses is complete. One that does either is bound by
everything below.

## Invariants

1. **A LAN record proves nothing.** A mesh discovery response is a signed
   control frame from the provider; a DNS-SD record is an unsigned multicast
   answer from whoever is on the segment. Anything an implementation derives
   from a LAN record carries `source: "lan"`, and an application MUST treat it
   as a claim rather than a discovery. The failure this prevents: an
   application that files a LAN import beside a mesh discovery lets any host
   on the LAN name any address as the provider of any service.
2. **A LAN record is never re-advertised on the mesh.** An implementation
   MUST NOT pass an imported record to `register_service` or otherwise
   publish it under its own identity. Imports go to the application's own
   registry and to the application's events, and nowhere else. The failure
   this prevents is a forgery laundered into a signature: a registration made
   on the strength of an anonymous LAN record would go out in signed discovery
   responses under this node's identity, and every mesh peer would trust the
   claim on the strength of that signature.
3. **Only this node's own registrations are published, once each.** One
   registration is one instance, named by the pair `(address, service_id)`,
   and a registration that leaves the local registry leaves the LAN.
4. **A field that does not fit is refused, never truncated.** A truncated
   service id names a different service; a truncated capability value is a
   different claim. An implementation that cannot publish a registration as
   specified publishes nothing for it and keeps the mesh registration.
5. **A service instance is not a peer hint.** The record carries this node's
   address so that an application knows whom to ask, and it may carry the
   port of a peer stream, but a browser looking for peers under
   [peer-stream framing](stream-framing.md#finding-a-peer-on-a-lan) MUST
   ignore any record that carries a `sid` entry. Without this rule every
   published service is a second, third and fourth connector to one host.

## The service type

The service type is `_offlineprotocol._tcp`, the same type the peer-stream
chapter fixes, and the one type this protocol registers: a service label is
at most fifteen letters, digits and hyphens (RFC 6763 section 7), and this
one is exactly fifteen. Service instances are distinguished from peer-stream
records by the subtype `_svc`, so that a service instance is published under

```
_svc._sub._offlineprotocol._tcp.<domain>
```

and a browser looking for services browses that subtype. RFC 6763 section
7.1 defines a subtype as an additional PTR to an instance that is listed
under its parent type, so a responder that follows it answers a browse of
the base type with service instances as well, which is why invariant 5
exists and why a peer browser keys on the `sid` entry rather than on the
name it was browsing. Some responders answer only the subtype browse
(python-zeroconf indexes an instance under the one type it was registered
with); a browser MUST NOT rely on either behaviour.

The domain is `local.` on a multicast LAN. Nothing here depends on it.

## The instance name

The instance name is `svc-` followed by the first sixteen hexadecimal digits,
lower case, of

```
SHA-256(address || 0x00 || service_id)
```

where `address` is the publisher's canonical address (`off1…`) and
`service_id` is the descriptor's id, both as UTF-8. Twenty octets, under the
63-octet bound RFC 6763 section 4.1.1 places on an instance name. The name
carries no meaning: the address and the id live in the TXT record, once each,
and a name that repeated them would be a second place to get them wrong. It is
a digest rather than a counter so that a re-publish after a restart claims the
same name and a responder's conflict resolution has nothing to resolve.

## The SRV record

The SRV target is a host name of the publisher's own, never the machine's
`<hostname>.local.`, which the operating system's responder may already
answer for. An implementation SHOULD reuse the host name its peer-stream
record uses when it publishes one, so that both resolve to one set of
addresses. The addresses are the publisher's interface addresses; a record
with none is seen by every browser and resolved by none.

The port is the publisher's peer-stream listening port when it listens, and
`0` when it does not. A browser MUST NOT connect to it on the strength of
this record (invariant 5); the port is there so that a host running both
records publishes one consistent SRV target, not as an endpoint.

## The TXT record

The record is a sequence of `key=value` strings in this order:

| Entry | Value | Required |
|-------|-------|----------|
| `txtvers=1` | The version of this mapping. First, as RFC 6763 section 6.7 recommends, so a reader can stop at the first string | Yes |
| `sid=<service_id>` | The descriptor's `service_id`, as UTF-8 | Yes |
| `ver=<version>` | The descriptor's `version`, as UTF-8; the engine does not parse it and neither does this mapping | Yes, possibly empty |
| `addr=<off1…>` | The publisher's canonical address, which is what a `service_discovered` event names as `provider_peer_id` | Yes |
| `c.<key>=<value>` | One string per capability, keys in the byte order of their lower-case form, so that one descriptor is one record | Zero or more |

Keys are case-insensitive, as RFC 6763 section 6.4 makes every TXT key:
`SID` is `sid`, and `c.Foo` and `c.foo` are one key. A reader folds a key
to lower case before it looks the key up, so an imported capability key is
the publisher's key in lower case. A publisher whose descriptor holds two
capability keys that are one key under case folding MUST refuse the
descriptor rather than publish either or both (invariant 4): a record with
one of them dropped is a different claim, and a record with both is the
duplicate below.

A reader MUST ignore a record whose `txtvers` is not `1`, and MUST ignore a
record missing `sid` or `addr`, or whose `addr` is not an address. Unknown
keys are ignored. A key that appears twice, under case folding, makes the
record malformed and it is ignored whole. RFC 6763 section 6.4 has a client
keep the first of a duplicated key and ignore the rest; this mapping is
stricter about what it accepts as its own record, in the way it is about
`txtvers`: a publisher under this chapter never emits a duplicate, and two
readers that kept different occurrences would disagree on which address a
record claims, so a duplicate marks a record that is not this mapping's. A
reader that applies the RFC's rule instead keeps the first occurrence and
never the last.

### Bounds

Every bound below is refused at the publisher, never truncated
(invariant 4). A reader applies the same bounds to what it imports, so that a
publisher that ignores them cannot make a reader hold what the reader would
never have published.

| Bound | Value | Where it comes from |
|-------|-------|---------------------|
| Service label | 15 characters | RFC 6763 section 7; `_offlineprotocol` is at the bound |
| Instance name | 63 octets | RFC 6763 section 4.1.1; `svc-` plus sixteen hex digits is 20 |
| One TXT string (`key=value`) | 255 bytes | RFC 6763 section 6.1, the length octet |
| TXT key | Printable ASCII (0x20 to 0x7E) with no `=`, at least one character | RFC 6763 section 6.4; a capability key must satisfy it or the descriptor is not publishable |
| `sid` value | 200 bytes | The engine allows 256, which does not fit one string beside `sid=`; 200 leaves the string within its bound with margin, and an id that long is not something a LAN needs to carry |
| Whole TXT record | 1300 bytes, counting one length octet per string | RFC 6763 section 6.2, so that the record fits one 1500-byte Ethernet frame with its headers |

A capability value is opaque bytes to DNS-SD and UTF-8 here; it is bounded
only by the string bound.

## Importing a record

A browser that resolves an instance under the subtype, checks the TXT record
against the table above and finds it well formed has a `ServiceDescriptor`
claimed by the address in `addr`. What it does with it is bounded by
invariants 1 and 2:

- It delivers a `service_discovered` event to the application with the fields
  the mesh event has (`query_id`, `service_id`, `version`,
  `provider_peer_id`, `capabilities`, `hop_count`) and one more,
  `source: "lan"`. `query_id` is empty, because no query was sent, and
  `hop_count` is `0`, because the record came from the segment. The mesh event
  is unchanged and carries no `source`; a reader that wants to distinguish the
  two checks for the field, as the bridge rules require of every additive
  field ([C3](../bridges/README.md#c3-events-cross-as-opaque-json)).
- It keeps the import in an application-level registry that the engine never
  reads. The engine's registry holds what this node offers; the import is
  what a neighbour claims to offer.
- It never registers the import with the engine (invariant 2).

An import that names this node's own address is ignored: it is this node's
own record coming back.

### Lifetime

A DNS-SD record has the lifetime its publisher gave it, and a responder that
dies without a goodbye leaves its records to age out in every browser's cache,
which for the PTR is 75 minutes by RFC 6762 section 10. An importer therefore
owes the application a shorter liveness rule of its own, and it keeps two
sets to hold it:

- **The browsed set** is every instance name the browser has reported and
  not yet reported gone. A name enters it when the browser reports the
  instance and leaves it only when the browser reports the instance gone.
  The importer re-resolves every browsed name at half its time to live,
  whether the name is listed or not.
- **The listed set** is what the application sees: every browsed name whose
  record resolved and was well formed within the importer's time to live.
  An entry leaves it when it has not resolved within that time, or when the
  browser reports the instance gone, whichever is first.

The two sets are kept apart because a browser reports an instance once: it
reports the instance again only when its own cache has forgotten the record,
which for the PTR is the 75 minutes above. An importer that stopped
re-resolving a name when the name left the listed set would lose a
neighbour that missed one resolve window until that cache turned over.

Delivery to the application is therefore at least once: an entry that
expires from the listed set and resolves again is announced again, with the
same event. An application that keys its own state on `(address,
service_id)` sees the second announcement as a refresh.

Removal is the importer dropping the entry from its own registry; it never
calls `unregister_service`: the engine never held the entry, and the id may
be one this node offers itself.

Nothing on the mesh side has a lifetime: a mesh registration stands until it
is unregistered, and a mesh discovery response is a point-in-time answer with
no removal signal. The importer's time to live is the one lifetime in the
system, and it belongs to the LAN side only.

## Publishing

A publisher publishes each registration in its local registry as one instance
and withdraws it when the registration is removed. A registration whose
descriptor cannot be laid out within the bounds is not published, and the
publisher says so in its log; the mesh registration is unaffected. Publishing
is best effort: an application that needs to know whether a service is
reachable over the LAN asks the registry, not the mesh.

What a device publishes on a LAN is visible to every device on it. That is
the same exposure as the mesh discovery response, which any querying peer
receives, and the same exposure as the peer-stream record: the address is
public by design, and the service list is what the node chose to advertise.
An application that does not want a service visible on the LAN does not
publish it there; the mapping is per registration, not all or nothing.

## Position among the discovery paths

| Path | Signed | Reach | Lifetime | Event field |
|------|--------|-------|----------|-------------|
| Mesh discovery response | Yes, by the provider | Every peer the gossip reaches, up to ten hops | None; a response is an answer, a registration stands until unregistered | No `source` |
| DNS-SD instance | No | The multicast segment | The importer's time to live, re-resolved at half of it | `source: "lan"` |

A LAN import can be confirmed by a mesh discovery: an application that
receives a LAN import and then a mesh response for the same `(address,
service_id)` has a signed answer, and may treat the LAN entry as confirmed
from then on. This chapter does not require an implementation to do that
join; it requires only that the two are never mistaken for each other.
