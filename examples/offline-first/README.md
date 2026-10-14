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
| 4 | Through the middle: A and C never meet, only B hears both | C receives A's message at `hop_count` 1 within 30 s, B reports `message_relayed`, and A gets `message_delivered` back the same way. On fresh identities the key exchange crosses B too |
| 5 | Carried for a sender that is off: C off, A sends, B takes it, A off, C on | C receives it at `hop_count` 1 within 60 s of coming back, while A is still off, so B held it; once A is back, its retry and acknowledgement queues drain |

The table `run.py` prints gives each scenario's wall-clock seconds from the
moment the recipient came back (1, 2 and 5) or from the send (4): about half
a second to a second in containers on one machine. The `latency_ms` a
receipt carries is the engine's own time from the last transmission to the
acknowledgement, tens of milliseconds there. Neither says what radios will
do. A receipt over the LAN peer stream names `wifiDirect`, the transport slot
the peer stream occupies.

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
python3 run.py            # builds the image, starts the devices, runs 1, 2, 4 and 5
python3 run.py --fresh    # new identities first: in 4, A and C have never met
python3 run.py --scenario 4 --scenario 5
```

The image installs the package from PyPI unless a Linux wheel built from a
checkout is in `bindings/python/docker/wheels/`, and its build fails when
the installed package has no `offline-protocol-verify`, which 0.29.0 is the
first release to ship. To run a checkout, put its wheel there (built as
[the Python README](../../bindings/python/README.md#a-linux-wheel-from-a-checkout-on-another-host)
says), or build the image another way and pass `--no-build` to use
`offline-protocol-service:offline-first` as it is. `run.py` writes `.env` with one store key per device the first
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
4. `pair` B and C, and A and B; `state --peer <C>` on A, to report whether
   A and C have met (`NoKeyPackage` when they never have); `watch` on A and
   B; `send` from A to C, and `await --until received` on C. The session
   between A and C, when there is none yet, forms through B while the
   message waits.
5. With A and C in session (one message through B forms it when they are
   not), `docker stop` C, `send` from A to C, and poll `state` on B until
   its `mesh_relay_stats.queued` counts one more; then `docker stop` A,
   `docker start` C, and `await --until received` on C. Last, `docker start`
   A and poll its `state` until `retry_queue_size` and `pending_ack_count`
   are both 0.

The order matters in one place, and it is the engine's rule rather than the
script's: a receipt that fires while no client is connected is not held, so
the wait on the sender starts before the recipient can answer. That is also
why 5 checks A's queues rather than awaiting its receipt: A is off when C
first acknowledges.

## On hardware

`run.py --ssh a=user@host --ssh b=user@host --ssh c=user@host` runs the same
checks over ssh against hosts that already run the service, and asks the
operator to switch devices off and on at each step. For 4 and 5, a and c
must never hear each other directly: each on its own segment that only b
joins. Each host runs the [service image](../../bindings/python/docker) under
host networking with `OP_LISTEN` set to its own LAN address, or the service
directly. The verifier has to run where the service's socket is: for the
image that is inside the container, so add `--remote-exec "docker exec
<container>"` (`docker-offline-protocol-1` for the image's `compose.yml`
started from its own directory); for the service run directly, add
`--socket` with the path it was started with.

The legs that need real radios or more than one machine, and how to run
each:

- **LAN between hosts.** Two or three boxes on one segment, `OP_LAN=1` so
  they find each other over DNS-SD. Scenarios 1, 2, 4 and 5 as above; for 4
  and 5, put C on a second segment that only B joins.
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

- **A message a direct link took and then lost crosses the mesh late**:
  a message sent down a stream to a device that went away without closing
  it is retried down that stream until keepalive ends it (about 30 s), and
  only the next retry is handed to the neighbours. No scenario here takes a
  link away under a live stream; scenario 3 on hardware does.
- **Every phone receives every notification on a Linux box.** A phone that
  writes the Bluetooth LE Hello (Android, and iOS from this release) is bound
  to its address on the box's peripheral however many centrals are connected.
  BlueZ offers no per-central notification, so on a Linux box every subscribed
  phone receives every fragment, and the core drops what is not addressed to
  it. An app built on an older SDK writes no hello and is mapped to its user id
  only while it is the one central connected.
- **An image built from 0.28.0 or earlier** fails scenarios 4 and 5: that
  engine forms no session between devices that reach each other only
  through a third, and drops an acknowledgement for a message whose only
  route was the mesh (#537). `--allow-missing-hop-receipt` covers only the
  second, for identities that already share a session.

## What has been run

| When | Where | Scenarios | Result |
|---|---|---|---|
| 2026-10-08 | `run.py`, Docker 28.3 on one arm64 laptop, the image built from the 0.28.0 Linux wheel with this branch's Python sources, so an engine without #537 | 1, 2, 4 | pass, twice in a row (receipt latency 29 to 45 ms; 4 without A's receipt, which that engine never sends) |
| 2026-10-08 | `run.py --fresh` then `run.py`, the same machine, the image with the native library built from `main` after #537 and this branch's Python sources | 1, 2, 4 | pass, twice in a row, A's receipt required in 4 and back at hop 1 (latency 33 to 36 ms in 1 and 2) |
| 2026-10-12 | `run.py --fresh`, `run.py` twice, and `run.py --fresh --scenario 5`, Docker 28.3 on one arm64 laptop, the image built by its Dockerfile from a Linux wheel of `main` (d43f4aba) | 1, 2, 4, 5 | pass on every run. 4 on fresh identities: A and C had never met, C received at hop 1 and A's receipt came back at hop 1, 0.9 s from the send. 5: C received from B 0.4 s after coming back, with A off; A's queues drained once it was back. Receipt latency 26 to 33 ms in 1 and 2 |

Nothing on this page has been run between separate hosts, over Bluetooth
LE, over a relay, through a gateway daemon, or with a phone.
