# Offline first: what the network does when a device is not there

A request with a deadline needs both devices on at once. A message does
not: the sender holds it on disk until the recipient can be reached, by
whatever carrier reaches it, through whichever devices are in between, and
the recipient's own acknowledgement comes back as `message_delivered`. These
scenarios show that, and each one passes or fails on the engine's events,
read on the device by
[`offline-protocol-verify`](../../docs/local-api.md#checking-what-the-network-did-offline-protocol-verify).

| # | Scenario | Passes when |
|---|---|---|
| 1 | Store and forward: B off, A sends, B on | B receives it, and A gets `message_delivered` within 60 s of B coming back |
| 2 | The sender restarts: B off, A sends, A killed (`SIGKILL`) and started, B on | the same, from the restarted A: the queue was on disk |
| 3 | The carrier changes: the link A and B were using goes away | every message of a `ping` run is delivered, and `message_delivered.transport` names the new carrier (hardware only, below) |
| 4 | Through the middle: A and C meet once, then only B hears both | C receives A's message at `hop_count` 1 within 30 s, and B reports `message_relayed` |

## In containers, on one machine

```
a ---- net-ab ---- b ---- net-bc ---- c
```

[`compose.yml`](compose.yml) runs three devices from the
[service image](../../bindings/python/docker), each with its own identity
on its own volume. a and c share no network, so b is the only way between
them. Peers are static, so the topology is the file's and not multicast
DNS's.

```bash
python3 run.py            # builds the image, starts the devices, runs 1, 2 and 4
python3 run.py --fresh    # new identities first
python3 run.py --scenario 4 --require-hop-receipt
```

The image installs the package from PyPI unless a Linux wheel built from a
checkout is in `bindings/python/docker/wheels/`, and its build fails when
the installed package has no `offline-protocol-verify`, which is true of
every release so far: until one ships, put a wheel there, or build the image
another way and pass `--no-build` to use `offline-protocol-service:offline-first`
as it is. `run.py` writes `.env` with one store key per device the first
time; keep it, or the devices come back with new addresses. It prints each
scenario as it finishes and a table at the end, and exits non-zero if any
failed.

What `run.py` does, so each step can be run by hand with `docker exec
offline-first-<device> offline-protocol-verify --socket /run/offline-protocol/api.sock ...`:

1. `pair` A and B (the session forms by itself once they hear each other),
   `docker stop` B, `send` from A, start `await <id> --until delivered` on
   A, `docker start` B, then `await <id> --until received` on B.
2. The same with `docker kill --signal KILL` and `docker start` on A between
   the send and B's return.
4. `docker network connect` C to net-ab and `pair` A and C; `docker stop` C,
   disconnect it, `docker start` it; then `watch` on A and B, `send` from A
   to C, and `await --until received` on C.

Two things in that order matter, and both are the engine's rules rather
than the script's. A receipt that fires while no client is connected is not
held, so the wait on the sender starts before the recipient can answer. And
C is stopped before it leaves net-ab: taken off the network first, it would
leave A a stream that looks open for about 30 s, and a message A sent down
it would never be handed to B (see the known gaps).

## On hardware

`run.py --ssh a=user@host --ssh b=user@host --ssh c=user@host` runs the same
checks over ssh against hosts that already run the service, and asks the
operator to switch devices off and on, and to move a and c apart, at each
step. Each host runs the [service image](../../bindings/python/docker) under
host networking with `OP_LISTEN` set to its own LAN address, or the service
directly. The verifier has to run where the service's socket is: for the
image that is inside the container, so add `--remote-exec "docker exec
<container>"` (`docker-offline-protocol-1` for the image's `compose.yml`
started from its own directory); for the service run directly, add
`--socket` with the path it was started with.

The legs that need real radios or more than one machine, and how to run
each:

- **LAN between hosts.** Two or three boxes on one segment, `OP_LAN=1` so
  they find each other over DNS-SD. Scenarios 1, 2, 4 as above; for 4, put
  C on a second segment that only B joins.
- **The carrier changes (3).** Two boxes with `OP_CONFIG=/etc/offline-protocol/config-ble.json`
  and the D-Bus grant, so both peer streams and Bluetooth LE are up. Start
  `offline-protocol-verify ping <B> --every 2 --count 60` on A, then pull
  the cable (or `ip link set <iface> down`) on B. Expect the messages in
  flight to arrive about 30 to 90 s late over `ble` (keepalive ends the dead
  stream in about 30 s, then the next retry picks the carrier that is up),
  and later ones over `ble` at once. Plug it back in to see `wifiDirect`
  return.
- **Relay.** The relay server with a Postgres database and authentication
  off for a closed test, `config-relay.json` and `OP_RELAY=ws://<relay>:3000/ws`
  on each box, and no peer stream between them (`OP_LAN=0` and no
  `OP_PEERS`, or two networks): a live direct link to the recipient is
  tried before any other carrier, so on one LAN the message comes back over
  `wifiDirect` and proves nothing about the relay. The receipt names
  `internet` when it does. Scenario 1 twice: once with A also off when B
  returns (the relay's mailbox holds the frame), once with the relay
  unreachable from A (A's outbox holds it). `message_undeliverable` is
  printed as status on the way: it is the relay saying B is away now, not a
  failure.
- **Reticulum.** A gateway daemon and `rnsd` on each of two boxes with a
  backbone between them (a TCP interface, or a pair of RNodes), a config
  with `reticulum_enabled` and `OP_GATEWAY=127.0.0.1:4242`, and no peer
  stream between the boxes, as for the relay. Check the attach
  first (the transport comes up only after the daemon's capabilities), then
  scenario 1. The service's gateway client has not met a real daemon yet.
- **A phone.** The React Native example app with Bluetooth LE on, against a
  box running `config-ble.json`: the box's `await --until received`, and
  the app's own delivered state, both directions. An iPhone can also reach
  a box over the LAN peer stream on the same Wi-Fi.

## Known gaps

- **The sender's receipt does not come back across a hop.** In scenario 4,
  C's acknowledgement is carried back by B and dropped by A: a message whose
  only route is the mesh has no pending acknowledgement on A to settle.
  `run.py` reports it and passes unless `--require-hop-receipt`. The engine
  fix is #537.
- **A message a direct link took and then lost never crosses the mesh.**
  Only a send that no carrier takes is handed to neighbours; a retry that
  the carriers refuse is queued again for direct carriers only. So a message
  sent down a stream to a device that went away without closing it waits
  for a direct link to that device.
- **Bluetooth LE on a Linux box as the peripheral** does not learn which
  phone wrote to it, so replies to a phone go over the box's central role.
  One Bluetooth LE peer per box until that is fixed.

## What has been run

| When | Where | Scenarios | Result |
|---|---|---|---|
| 2026-10-08 | `run.py`, Docker 28.3 on one arm64 laptop, the image built from the 0.28.0 Linux wheel with this branch's Python sources (no native change since 0.28.0) | 1, 2, 4 | pass, twice in a row (receipt latency 29 to 45 ms; 4 without A's receipt, as above) |

Nothing on this page has been run between separate hosts, over Bluetooth
LE, over a relay, through a gateway daemon, or with a phone.
