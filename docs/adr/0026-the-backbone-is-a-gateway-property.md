# 0026. The backbone is a gateway property

**Status:** Accepted

## Context

The [gateway contract](../spec/gateway-contract.md) was written when one
backbone existed. Its backbone section opened with "between gateways, the
wide-area carrier is Reticulum", the Verdict rule was stated for "a Reticulum
gateway", and the one capability token a gateway advertised for its backbone
was `backbone_reticulum_v1`, with no rule for a second. The SDK's own names
followed: the transport slot, the `reticulum_*` FFI entry points and the two
mobile managers are all named after that backbone, and their doc comments
said "the attached Reticulum gateway" for what is, on the wire, a daemon
reached over local IP.

None of that is what the device does. Four facts, verified in the tree:

**The device never touches a backbone.** The transport opens no backbone
link, holds no backbone identity and sees no backbone frame. The bundled
managers speak newline-delimited JSON to a daemon at a configurable local
address, and everything past the daemon is the daemon's.

**The engine stores the gateway's tokens and reads none of them.** The
capability set a gateway advertises at attach is kept, bounded and cleared on
drop, and the only readers of the set are tests. No send path, no parking
decision and no transport choice consults a token. The `mailbox_v1`
behaviour that looks like a token-driven feature is driven by the `stored`
and `pushed` flags on each verdict, and the token is the gateway's promise to
honour them, not the switch.

**Every gateway answer is recorded against a transport type.** The
reachability facts a verdict or a presence answer produce are keyed by
recipient and by `TransportType`, and the newest answer for a carrier
replaces the last. The engine holds one transport per type, and the FFI
holds one daemon connection state. Two daemons attached through the one
slot would overwrite each other's facts recipient by recipient, and neither
would be wrong on its own terms.

**Forty-odd guards pin the names.** Source guards in the FFI crate read the
mobile managers by path and assert their shape, and the `reticulum_*` entry
points are the generated API of three bindings. Renaming the slot is a
three-language break for a property the device does not have.

## Decision

1. **The backbone is the gateway's.** The contract states what a backbone
   owes a gateway (gateway-to-gateway presence, arbitrary-size framing, a
   provisioned peer list, a declared rate class) as obligations on the
   gateway, with Reticulum as the reference backbone in a subsection of its
   own. Nothing a device does depends on the section.
2. **The device learns the backbone only as a token.** A gateway advertises
   each backbone it reaches as one token of the family `backbone_<kind>_v1`.
   A client ignores a kind it does not know and keeps the session. A
   gateway may advertise no backbone token, and is a conforming gateway.
   `backbone_reticulum_v1` is the first member, unchanged in spelling.
3. **The token is advisory and never a routing input.** The engine keeps
   storing the set and keeps reading no token from it. A device does not
   choose a carrier, park a message or size a frame because of a backbone
   token. A token is set by whoever holds the socket, so a routing choice
   that trusts it is a routing choice the gateway makes for the device,
   and invariant 3 of the contract (no facts, no change) forbids that.
4. **One attached gateway daemon at a time.** The daemon link is one carrier
   however many backbones stand behind it, and the facts it produces are
   keyed by that carrier. A second daemon on the same device needs a second
   transport type, which is a change to the transport set (a sixth variant,
   every exhaustive match, the selector's ranking, the bridges' managers)
   and is deferred until a deployment needs two daemons on one device. A
   device that wants a second gateway's reach gets it through the first: the
   gateways are peers over the backbone.
5. **The names stay.** The `reticulum_*` FFI entry points, the transport
   variant and the manager file names keep their spelling. They name the
   transport slot, after its reference backbone. Their doc comments say
   what the slot is: the gateway daemon carrier, whose backbone is the
   gateway's property.

## Consequences

- A gateway on another backbone, or on none, attaches the same devices with
  the same managers and no SDK change. What it advertises differs by one
  token, which nothing reads.
- The threat model's abuse-budget argument for gateways is per rate class:
  the budgets a gateway applies are sized to what its backbone can carry,
  and a device cannot verify either the class or the budgets. That was
  already true for one backbone and is now stated for any.
- The `reticulum_*` names remain a small standing confusion for a reader
  who meets them first. The doc comments carry the correction, and a rename
  is future debt that would cost a three-binding API break to pay.
- Two daemons on one device is not supported, and a bridge that opens a
  second connection to a second daemon while the first is attached produces
  facts that overwrite each other. The contract says so. Nothing enforces
  it in the engine, because the engine sees one carrier.

## Alternatives considered

**A transport variant per backbone kind.** Honest about what a second daemon
would need, and wrong about what a device does: the device speaks one
protocol to a daemon whatever stands behind it, so a variant per kind would
put a property the device cannot observe into an enum the device branches
on, and every exhaustive match would grow for a distinction with no
behaviour behind it.

**Rename the slot to something neutral now.** The right name, at the cost of
a three-binding API break, forty guard edits and two manager renames, for a
change with no behaviour in it. The doc comments carry the correction
instead, and the rename waits for an API break that is happening anyway.

**Carry the backbone in a wire field rather than a token.** A field is a
promise that something reads it. Nothing should, for the reason in decision
3, and a token in a set that is already bounded, cleared on drop and
documented as never driving a security decision is the shape that makes that
hard to forget.

## What would undo this

A send path, a parking decision or a transport ranking that reads a
`backbone_*` token. The getter exists for tests and for an application that
wants to show the token; the first production reader turns an advisory
string set by the socket's holder into a routing input, and does so without
anything failing.

A second daemon attached through the one slot. It works, in the sense that
frames flow, and the two gateways' verdicts overwrite each other from then
on.

A backbone obligation restated in device terms, such as a device-side frame
ceiling sized to a backbone's MDU. The device does not know what the
backbone is, and the day it is told, every device is coupled to one
deployment's link.
