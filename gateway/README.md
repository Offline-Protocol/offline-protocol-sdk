# Offline Protocol gateway daemon

The reference implementation of the gateway half of
[the gateway contract](../docs/spec/gateway-contract.md). Devices running the
SDK attach to it over local IP through the transport the SDK calls
`reticulum` (the gateway slot,
[ADR 0026](../docs/adr/0026-the-backbone-is-a-gateway-property.md)). It carries
their sealed frames to each other and, over a Reticulum backbone, to devices
attached to other gateways. That backbone is how the SDK reaches LoRa: an
RNode on each gateway, and Reticulum between them.

```
phone ─Wi-Fi─► [gateway A + RNode] ~~~ LoRa (Reticulum) ~~~ [gateway B + RNode] ◄─Wi-Fi─ phone
```

Nothing in the SDK changes for this. A device enables the `reticulum`
transport, points it at the gateway's address, and sees one more carrier. It
never learns that LoRa is behind the gateway, except as a
`backbone_reticulum_v1` capability token it does not act on.

## What it guarantees

These are the contract's rules, and the tests in `tests/` pin each one.

- **A session is bound only by a proof this gateway checked.** The device
  signs its address and a 32-byte challenge minted for this connection. The
  daemon verifies it with the SDK's own `verify_gateway_address_proof` (the
  signature, and that the key derives to the address) and spends the
  challenge, so a captured declaration cannot be replayed.
- **Every submission gets exactly one verdict**, correlated by the client's
  id. `recipient_unreachable` when no gateway this one can reach has the
  recipient; a plain failure (`budget_exceeded`, `frame_too_large`,
  `backbone_timeout`, `attach_required`, `malformed_request`) otherwise.
- **A verdict is a claim, never settlement.** Only the recipient's end-to-end
  acknowledgement settles a message. The daemon holds nothing and promises
  nothing it does not do: it sets neither `pushed` nor `stored`.
- **Ciphertext only.** The daemon sees MLS ciphertext and routing metadata,
  on the device link and on the backbone.
- **Budgets.** As token buckets: submissions per device; on a scarce
  backbone, bytes per source host and bytes per peer gateway on the way out
  (shared by every device, since the airtime toward a peer is the scarce
  thing, and validated to stay above the per-host one); and frames per peer
  gateway on the way in. Every burst holds a first contact (two engines
  pairing and exchanging one message send 18 to 20 frames, about 16 KB, the
  largest about 4 KB).
- **One request per peer at a time.** Reticulum cannot withdraw a transfer
  it has queued, so the daemon queues instead: a frame whose time runs out
  before its turn is answered `backbone_timeout` without reaching the radio.
- **Sessions.** Sockets that never attach have their own pool, and a
  newcomer evicts the oldest of them, so they cannot lock devices out. One
  host (one source address) may hold at most 8 sessions. Devices behind NAT
  or a proxy are one host, so put the gateway on the network the devices
  join, not behind either.

What it does not do yet: hold frames for an absent recipient (no
`mailbox_v1`), push to a sleeping phone, or re-originate backbone frames into
a Bluetooth zone. A device has to be attached to a gateway over IP to be
reached through it. When the recipient is not attached, the device's own
parking and mesh fallback apply, as they do with no gateway at all.

## Install

Python 3.11 or later. The SDK's Python package supplies the verifier; until a
release carries `verify_gateway_address_proof`, install the build from this
repository.

```bash
cd gateway
python -m venv .venv
bash ../bindings/python/scripts/build-desktop.sh --release
.venv/bin/pip install -e ../bindings/python -e '.[reticulum,dev]'
```

## Run a local gateway

With no `[reticulum]` section the daemon is a local gateway: every device
attached to it can reach every other, and nothing leaves the box.

```bash
.venv/bin/offline-gateway --listen 0.0.0.0:4242
```

On each device, enable the `reticulum` transport and set its daemon address
to the gateway's `HOST:PORT`:

- React Native: `transports: { reticulum: { enabled: true, daemonAddress: "192.168.4.1:4242" } }`
- iOS and Android natively: `ReticulumManager.configure(daemonAddress: ...)`
- Python: `GatewayManager.configure(daemon_address=...)`, or `--gateway` for
  the Python service

The iOS and Android managers split the address on `:`, so give them an IPv4
address or a host name, not an IPv6 literal.

## Run a LoRa backbone

1. **Flash a radio for each gateway.** Any RNode-compatible LoRa board:
   `pip install rns`, plug it in, `rnodeconf --autoinstall`.
2. **Copy `examples/` to each gateway box** and set the radio in
   `rns/config`: the port, and parameters that are legal where you are and
   identical on every gateway. The example is EU868 with duty-cycle caps.
3. **Print each gateway's destination hash.**
   `offline-gateway --config gateway.toml --print-destination` creates the
   gateway's identity on first use and prints the hash its peers provision.
4. **Provision the peers.** Put each other gateway's hash in `peers`.
   Gateways are provisioned, never discovered
   ([ADR 0016](../docs/adr/0016-gateways-are-provisioned-not-emergent.md)):
   a link from an unlisted gateway is torn down, and its requests refused.
5. **Start it.** `offline-gateway --config gateway.toml`. On its first start a
   gateway mints its announce stamp, a one-time proof of work Reticulum
   requires before it announces. That takes seconds on a laptop and minutes
   on a small board. The stamp is stored, so later starts announce at once.
   The log says when each backbone link comes up.

How it uses Reticulum, in short: one destination per gateway
(`offline_gateway.backbone.v1`) under the gateway's own identity; one link
dialled by each side; frames as link requests, which Reticulum carries as
resources when they outgrow a packet; and presence as a snapshot when a link
comes up, then pushed changes, coalesced over two seconds. The module
docstring in `offline_gateway/reticulum.py` has the request shapes.

## What to expect over LoRa

LoRa is slow, and the SDK's messages are not small. One sealed direct message
is about 472 bytes on the wire at best, already more than one Reticulum
packet, and a first contact between two devices that have never met is
about 20 frames each way: connection request, key packages, the MLS Welcome,
acknowledgements. That last number is measured (`test_end_to_end.py`); the
rest of this section is arithmetic from the radio settings, not a
measurement over radios.

With the example radio (`examples/rns/config`: SF8 at 125 kHz, about 3.1
kbps, on the EU sub-band that allows 10 % airtime):

- a full packet is about 1.3 s on air, and a sealed frame is three or so
  transmissions from the sender, plus the peer's answers on its own airtime;
- 10 % is about 6 minutes of airtime an hour, so a gateway sustains about
  1.5 KB a minute toward a peer once Reticulum's overhead is paid, which is
  what the default `peer_egress_bytes_per_minute` allows;
- a frame has to cross inside the 45 s verdict timeout, which the 50 % short
  airtime cap allows up to about 6 KB: the default
  `max_backbone_frame_bytes`, and enough for the largest frame of a pairing;
- a first contact (about 16 KB each way) takes a few minutes, and a later
  message tens of seconds.

At 1 % airtime, the rule on most of the EU 868 MHz band, everything is ten
times slower: a first contact takes hours. Divide the egress budget
by ten there, or the device's frames are accepted faster than the radio can
carry them.

The daemon answers each submission within 45 seconds (the clients give up
at 60). On a slow or busy link a frame may be failed `backbone_timeout` and
retried by the device. Nothing is lost by that, only repeated, and the
timed-out transfer is cancelled, so the retry does not run beside it.

## Tests

```bash
.venv/bin/python -m pytest
```

- `test_gateway.py`: the device-facing contract, one rule at a time, with a
  stand-in verifier.
- `test_backbone.py`: two gateways over the in-process backbone.
- `test_end_to_end.py`: real SDK engines, attached with the SDK's own
  `GatewayManager`, pair over MLS and talk through one gateway and across two.
- `test_reticulum.py`: the same across two daemon processes joined by a real
  Reticulum backbone over TCP, which is the LoRa deployment with the radio
  swapped out. It lowers Reticulum's announce stamp so it does not wait for
  two mints.
- `test_reticulum_unit.py`: the backbone's handler, presence and framing
  rules, without a Reticulum instance.
