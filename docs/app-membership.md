# Application membership

For an application whose users must be visible only to each other while every
device running the SDK still carries everyone's traffic.

Every SDK application shares one radio neighbourhood: a phone running your
application relays frames for a phone running someone else's, and the denser
network is better for both. Application membership keeps that density and
takes away what it used to cost. With a publisher key configured, your
application sees only your users, answers only your users, and names itself
to no one else on the air. Other applications still carry your frames without
learning that your users are there.

The design and its reasons are in
[ADR 0037](adr/0037-an-applications-users-are-visible-only-to-that-application.md).
What it does and does not protect against is in the
[threat model](security/threat-model.md#r33-whoever-holds-an-applications-binary-can-recognise-its-users).

## What your application sees

With a publisher key configured:

- **Neighbours:** `neighbor_discovered`, `neighbor_lost` and RSSI readings
  report only peers that proved membership over the link, or peers your
  application already has a relation with (below). A peer that has not
  proved membership yet is held back and reported the moment it does.
- **Inbound frames:** a frame from a peer your application has not admitted
  is dropped before anything is done with it. It gets no acknowledgement, no
  event and no reply, exactly as if the sender were blocked.
- **Relaying is unchanged:** a frame addressed to someone else is carried as
  before, whoever it belongs to.
- **On the wire**, every frame your node originates names the application
  `offline`. The real application id travels sealed inside the message, and
  `message_received` reports it as before.
- **On the air**, the Bluetooth LE App tag, the Android Wi-Fi Direct group
  and the browser's nearby room come from your publisher key instead of your
  application id, so none of them names your application.
- **Service discovery** is sealed: only your users can read what is offered or
  asked for, and a provider answers only a query one of your users made.
  Configured and unconfigured builds do not discover each other's services.

A peer is admitted when any of these holds:

- it proved membership over a direct link;
- your application named it, by sending to it, starting a session with it or
  inviting it to a group (the set is persisted and bounded at 4096 peers);
- it already holds a session with this node (so turning membership on does
  not cut off existing contacts);
- it is on the roster of a group this node holds;
- it is several hops away and its key package or Welcome carries a valid
  end-to-end membership proof, which the SDK attaches for you.

With no publisher key configured, nothing on this page applies and the SDK
behaves as it always has.

## Set it up

### 1. Generate the publisher key pair, once, offline

The publisher key is an Ed25519 key pair that belongs to your application,
not to a device. Generate it on a machine you control:

```bash
openssl genpkey -algorithm ed25519 -out publisher.pem
# The 32-byte public key, which ships in your application:
openssl pkey -in publisher.pem -pubout -outform DER | tail -c 32 > publisher.pub
# The 32-byte private seed, which signs certificates:
openssl pkey -in publisher.pem -outform DER | tail -c 32 > publisher.key
```

Keep `publisher.key` where your signing backend can reach it and nowhere else.
The SDK never generates, stores or asks for it.

### 2. Ship the public key

Set `app_publisher_key` in the configuration to the 32 public key bytes. Every
build of your application that should see the others needs the same key. A
configured build and an unconfigured build of the same application are
strangers to each other, so turn it on in one release.

```rust
let config = ProtocolConfig::builder("com.example.chat", "alice")
    .app_publisher_key(PUBLISHER_PUBLIC_KEY)
    .build()?;
```

The bindings take the same field (`appPublisherKey` in Swift and Kotlin,
`app_publisher_key` in Python). A key that is not 32 bytes is refused with
`InvalidConfiguration`.

### 3. Give each device a certificate

A certificate is 93 bytes that bind one device's address to your application
until a time you choose. Your backend issues it:

1. After `initialize_mls`, the application reads its own address
   (`local_address`) and sends it to your backend.
2. The backend decides whether to enrol this device (this decision is your
   security boundary; see below) and signs a certificate.
3. The application hands the certificate to `set_app_membership` on every
   launch. The SDK holds it in memory only.

On the backend, the Python package signs one with the same function the SDK
uses:

```python
from offline_protocol_sdk import sign_app_membership

def issue(address: str, not_after: int) -> bytes:
    seed = open("publisher.key", "rb").read()
    return bytes(sign_app_membership(list(seed), address, not_after))
```

`not_after` is the last Unix second the certificate is current. A few days to
a few weeks is a good range: long enough to survive a long time offline,
short enough that a device you stop enrolling drops out on its own.

`set_app_membership` checks the certificate before it accepts it. It must
name this node's address, verify under the configured publisher key and be
current, and anything else is `InvalidArgument`. Handing it the certificate
it already holds does nothing. `app_membership_status` reports whether
membership is configured, when the current certificate expires (`None` once
it has), and how many neighbours are visible as members. When the expiry
draws near, fetch a new certificate.

### What the backend's decision means

The backend does not need to prove that the device owns the address: a
certificate for an address a device does not hold is useless to it, because
every frame and every exchange is signed by that address's identity key.
What the backend does decide is who counts as your user. Anyone who can run
your application can ask for a certificate for an address of their own, so
your enrolment policy (who you sign for, and whom you stop renewing) is the
boundary. Tie it to whatever already says someone is your user: an account,
a purchase, an invitation.

## Embedded signing, and what it gives away

For an application with no backend, `sign_app_membership` can run on the
device itself, with the private seed shipped inside the application:

```rust
let address = protocol.local_address().expect("after initialize_mls").to_string();
let cert = sign_app_membership(&PUBLISHER_PRIVATE_KEY, &address, not_after)?;
protocol.set_app_membership(&cert)?;
```

The function keeps no state and never logs, stores or sends the key. But an
application that embeds its publisher private key lets anyone holding the
application binary mint members. For that application, another application
built from your binary can pass as yours, not just recognise your users. The
protection against an application that does not have your binary is
unchanged.

Use embedded signing for a prototype, or for an application where being
recognised and impersonated by someone who unpacked it is acceptable. Move to
backend signing before that stops being true. The certificate format is the
same in both modes, so the move is one release: certificates issued on the
device stay valid until their `not_after`, and rotating the key pair cuts them
off at once.

## Leaf devices

A leaf (a microcontroller running `offline-protocol-leaf`) has no backend to
ask, so the phone that paired it provisions it:

1. Pair as the leaf chapter describes, but begin by asking for a session with
   the address the out-of-band anchor verified (`establish_secure_session`).
   With a publisher key the phone answers no stranger, and an unprovisioned
   leaf proves no membership, so that call is what lets the leaf's key
   package in.
2. Get a certificate for the leaf's address, from your backend or
   `sign_app_membership`, and hand it to
   `provision_leaf_membership(leaf_address, certificate)`. The phone seals it
   with the publisher key and its clock inside the pair's session; the leaf
   stores it before acknowledging.
3. From then on the leaf is a member like any other device, visible to every
   member it meets, not only the phone that paired it. Calling
   `provision_leaf_membership` again refreshes the leaf's notion of time,
   which is the only clock it checks certificates against.

A leaf nobody provisions behaves as it did before.

## Rotating the key pair

A new publisher key derives a new application secret, so every device on the
old key becomes a stranger to every device on the new one. Rotate:

- **when the private key may have leaked**, at once, in a release that ships
  the new public key, with the backend signing under the new key only;
- **when moving off embedded signing**, if the embedded key must stop working
  before its certificates expire.

Losing the private key without a leak is an outage, not a breach: existing
certificates keep working until `not_after`, and no new ones can be issued
until you rotate.

## What it does not hide

- **Addresses.** Carriers route on them. A device in range sees that an SDK
  node with some address is there, but nothing that names your application or
  groups your users.
- **Traffic shape.** Frame sizes and timing still show that two devices talk.
- **From someone holding your binary**, your users can be recognised, one
  connection at a time. Whatever your users use to recognise each other, a
  copy of your application holds too. With backend signing it cannot pass as
  one of your users unless your backend enrols it.
