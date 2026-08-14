# Group protocol

Groups are MLS groups (RFC 9420). This document specifies what this protocol
adds on top: how frames are addressed, how membership changes are judged, how
identities in the ratchet tree are bound to addresses, and how a message reaches
members who are not currently reachable over the mesh.

## Frames

| Prefix | Content |
|--------|---------|
| `__GRP_MLS_WELCOME__` | An MLS Welcome plus the invite metadata below |
| `__GRP_MLS_COMMIT__` | An MLS Commit for a membership change |
| `__GRP_MLS_MSG__` | An MLS application message |
| `__GRP_MLS_LEAVE__` | A leave notification |
| `__GRP_ROLE_CHG__` | An administrative role change |
| `__GRP_RENAME__` | A group rename |

### Welcome metadata

Beyond the MLS Welcome itself, the invite carries:

- **`created_by`**: the inviter's creator of record for the group. Adopted
  **first-write-wins** by the joiner. This is what makes the administrative
  creator fallback reachable for joiners when a role snapshot arrives
  incomplete.
- **`member_rich`**: a capability map for the existing roster, so a joiner can
  seal rich payloads to members it has never exchanged key packages with.
  Bounded to the joined MLS roster.

### Commit metadata

- **`affected_member_rich`**: the added member's capability, delivered to
  existing members. Admin-gated on the commit, like the role field.

## Leaf identity binding

This is the application-side Authentication Service that RFC 9420 sections
5.3.1 and 7.3 assign to the application. **MLS does not do it for you.**

### The rule

Every leaf entering local group state MUST carry the address that its **own
signature key** derives to, using the derivation in [Identity](identity.md).

### Why it is mandatory

An MLS basic credential is a bare self-asserted string. The wire-sender to
credential comparison that authenticates group application messages therefore
proves only that the forger typed the name they wanted, unless something binds
the credential to the key. On the ungated group data plane that costs a forger
no signature from anyone.

### Three seams, and they are not redundant

| Seam | When | Scope | Covers |
|------|------|-------|--------|
| Welcome | Before joining | The **whole** ratchet tree | The inviter chooses the tree wholesale |
| Commit | Pre-merge | Every credential the commit introduces or changes | New and renamed leaves |
| Use | At the sender check | The sending leaf, resolved by index | A leaf that entered by neither gate |

The Welcome walk is all-or-nothing by necessity. Joining while skipping bad
leaves would leave the joiner at an epoch computed over the **full** tree,
decrypting nothing.

The use-time check is the only one that covers a leaf written directly into an
implementation's own key store, bypassing both entry gates. Import-time
validation plus use-time validation is the same pairing key package handling
uses.

### The commit walk covers four sources

Not two. An implementation that walks only Add and Update proposals leaves the
**cheapest** attack open.

1. **The update path leaf.** A member renames their own leaf to a peer's
   address. No new leaf, no invite needed. Dropping this source is the gap.
2. **Update proposals.**
3. **Add proposals.**
4. **Group context extensions, specifically external senders.** Refused
   outright, as are all non-member senders. This protocol issues no external
   commits and no external proposals.

Source 2 is unreachable in this protocol today, and is kept deliberately: by
value, MLS attributes the proposal to the committer and forbids committing your
own update; by reference, the receiver must be holding the proposal, and this
protocol drops received proposal messages rather than storing them. A
propose-only API would make the loop live.

### Non-address credentials are refused, never skipped

"Nothing to derive, so pass" is the bypass. A credential that is not an address
is refused.

### Unconditional, unlike administrative enforcement

The verdict is computed from the commit's own bytes, so every honest member
reaches the same answer and a refusal forks the **attacker** off a group that
stays consistent. This is the property that administrative enforcement lacks,
and it is why this check is unconditional while that one is opt-in.

### Refusal dispositions

| Frame | Disposition | Why |
|-------|-------------|-----|
| `__GRP_MLS_MSG__` | No acknowledgement, identifier unmarked, never buffered | An acknowledgement confirms to an injector that the target is live |
| `__GRP_MLS_WELCOME__` | Consumed, so acknowledged and dedup-marked | Signature-gated, so the acknowledgement tells an unauthenticated injector nothing, and a permanent refusal should not be retransmitted |
| `__GRP_MLS_COMMIT__` | Consumed, as above | Same |

A refused commit MUST be classified **permanently** refused. An implementation
that treats it as retriable buffers it, re-decrypts it on every drain, and,
because a buffered commit that expires having been retried reads as an epoch
fork, turns one forged commit into a group-wide key update round plus a false
fork report.

### Roster reads

A roster read skips unbound leaves and **counts** them. The count is reported;
the claimed identities are not, because handing an attacker-chosen string back
through a second field returns it by another door.

The roster is not cosmetic. It addresses per-member fan-out, feeds the rich
payload gate, and supplies the address-ordered tiebreakers. A non-zero unbound
count is reported as a security warning, because this is the only seam at which
a leaf already seated in local state surfaces, and a log line reaches no
application.

That report differs in kind from the others: no frame was refused and no peer
delivered it, so it names **this device** as the subject rather than a
blameworthy peer, and the remedy it implies is to abandon the group rather than
to evict a member. The leaf cannot speak, but it holds live group secrets and
reads everything, which no later refusal undoes.

### Removal removes every matching leaf

Removing a member removes **all** leaves matching that address, not the first.

Through the wire gates a duplicate is unreachable, because MLS requires unique
signature keys and the binding ties credential to key. That argument covers the
gates, not the tree: a forged leaf written straight into a key store claims a
peer's address while carrying the attacker's key, violates no uniqueness rule,
and sits beside the victim's real leaf. First-match removal would leave the peer
in the group holding live keys while every roster read shows them gone.

## Membership authorization

### Report by default

MLS Add and Remove commits are applied by every receiving member with no
administrative check **by design**.

Rejecting a commit means declining the merge, which forks you permanently from
everyone who accepted it. The administrative overlay replicates best-effort:
roles ride on unreconciled notifications, and joiners get a point-in-time
snapshot. A merely lagging member would therefore partition itself with no
attacker involved.

Unauthorized changes are **reported** instead. Reports are rate-limited per
group and committer.

### The tri-state authorization field

Roster change events carry a three-valued authorization field:

| Value | Meaning |
|-------|---------|
| checked and authorized | A check ran and passed |
| checked and unauthorized | A check ran and failed |
| not evaluated | No check ran: own Welcome join, relay reconciliation |

An implementation MUST NOT emit "authorized" from a path that ran no check. The
third state exists precisely so that path has something honest to say.

### The delta is derived only when both roster reads succeed

The membership delta comes from a pre-commit roster read and a post-merge roster
read. If **either** fails, all delta-derived work is skipped.

A silent empty default on a failed read fabricates a full-roster delta and a
report naming an innocent committer.

The pre-commit roster MUST be MLS-derived, never taken from a members cache.
Relay reconciliation splices entries into that cache that were never in the
tree.

### Opt-in rejection

Rejection is available as an explicit opt-in, default off.

It runs **pre-merge at the single decryption chokepoint**, not in the commit
handler. Gating only the commit handler leaves two bypasses: a commit reframed
as an application message, and an `__MLS_ENC__` envelope naming a group
identifier.

**The fail-open rule is load-bearing.** Merge anyway when:

- the commit proposes no membership change,
- the identifier names a 1:1 session,
- group metadata is unreadable or absent,
- **the administrative set is not known to be non-empty.**

Reject only when the administrative set is known non-empty and a principal (the
committer, plus every proposal's sender) is positively not in it.

The creator of record is deliberately **not** consulted here. One
unauthenticated claim is too thin a basis to fork over.

Enforcement can detect an **absent** administrative view, never a **divergent**
one. That is why it is opt-in and never a fleet-wide default.

## Group message delivery

Two paths. The choice is made per send.

### Per-member fan-out

The MLS ciphertext is sent as one ordinary directed message per member.

This inherits the entire direct-message delivery ladder: the outbox, the
acknowledgement and retry machinery, relay write acknowledgement, offline push
carrying ciphertext, parking, probing, flushing, and the receiver's deferred
acknowledgement handling.

It costs O(N) frames. That does not risk a relay rate limiter at any group size
if the client meters relay-bound frames with a bucket strictly tighter than the
server's and **defers** rather than drops on exhaustion, so the fan-out
self-paces.

The real cost is drain latency. With a client bucket of 28 tokens refilling at
9 per second, frame N reaches the wire at roughly `(N - 28) / 9` seconds. Since
the acknowledgement timer starts at local enqueue, past roughly 118 members the
tail exceeds a 10 second acknowledgement timeout and is retransmitted before it
was ever written. Those duplicates are absorbed by deduplication, so this is
wasted work, not loss. Shared traffic on the same bucket lowers the threshold.

### Relay broadcast

One frame to the relay, which fans out server-side.

Taken only when **all four** hold:

1. broadcast is enabled in configuration,
2. the group roster is registered with the relay,
3. the relay advertised the `group_delivery_v3` capability,
4. a live check confirms internet availability.

#### Why the capability token is v3 and not v2

v3 is v2's settled-report contract **plus an address-aware relay group path**.
A v2 relay MUST fail the gate closed, because its username-keyed path and
address identity cannot compose:

- it cannot route to address-registered members,
- its report names members in a namespace that never intersects the MLS roster,
  so the set difference re-issues to **everyone** after every broadcast,
- any copy it does deliver arrives attributed by username, which fails the
  wire-sender to credential match **after** the decrypt already spent the
  ciphertext's ratchet generation.

That last one is the reason the **gate** is the fix rather than any
receiver-side cleanup. The generation burn is unrecoverable on the client:
MLS implementations persist message secrets through the storage provider before
the identity check runs, and skipping the group save does not undo it.

#### The delivery report contract

What makes a server-side broadcast safe to default on is that it is not
fire-and-forget.

1. The sender mints a **logical message identifier** and carries it in the
   broadcast frame. The relay stamps it onto its fan-out verbatim, or mints one
   if absent.
2. The sender arms a pending tracker keyed by that identifier.
3. The relay returns a **settled** report naming delivered members, pushed
   members, and missed members with opaque reasons. It can arrive up to roughly
   45 seconds later, so the tracker timeout is 60 seconds.
4. On receipt, the sender re-sends per-member copies to
   `roster − delivered − pushed − self`.

Step 4 covers both the reported misses **and members the relay never knew**,
because the relay's registered roster can be a strict subset of the MLS roster.

The report MUST arrive through a **dedicated entry point**, not by injecting a
message-plane frame. Message-plane injection makes the report forgeable by
anything that can reach the injector.

#### Failure handling

| Failure | Response |
|---------|----------|
| Report never arrives | Re-broadcast under the **same** logical identifier, up to 3 total sends, then downgrade to full per-member fan-out |
| Internet drops | Downgrade all pending broadcasts immediately |
| Tracker overflows (64 pending) | Downgrade the oldest to per-member |

Re-broadcasting under the same identifier is what makes retries safe: the relay
echoes it, so receiver deduplication and push deduplication both hold across
attempts.

**Known gap:** the tracker is memory-only, so a process kill inside the report
window loses the backstop.

#### Receiver-side identifier discipline

Re-issued copies carry the logical identifier. Handling it correctly requires
two **opposite** rules on the two paths, and getting either backwards causes
silent loss.

**Mesh path: mark only after a successful decrypt.**

The logical identifier is checked in the duplicate branch, which absorbs
cross-path duplicates without an MLS decrypt. A spent-generation decrypt would
misclassify as retriable and buffer noise. But the identifier is marked only
after a decrypt succeeds, because a failed decrypt must not poison it.

**Relay path: mark at arrival, before decrypt.**

Here the relay-supplied identifier **is** the logical identifier, and marking it
pre-decrypt is the replay-amplification defence: one MLS operation per
identifier.

That inversion carries an obligation. **Every arm that ends with the frame
neither delivered, nor buffered, nor consumed by MLS MUST unmark before
returning.** Security rejections, hard failures, and the plaintext-spoof drop
whose identifier is attacker-chosen wire input.

Otherwise a *rejected* copy reads as *delivered* to the duplicate check, and the
per-member re-issue that is the broadcast's own safety net is absorbed as a
cross-path duplicate and acknowledged: delivered nowhere, sender told delivered.

The obligation extends to the buffered-message drain, and that half is not
optional. A relay copy can outrun its Welcome, so it buffers **before** any
decrypt and its misattribution is judged on the drain rather than at arrival, an
ordering a hostile relay picks for free.

Unmarking cannot resurrect a burned generation. The honest recovered outcome is
"buffered and unacknowledged, custody with the sender", never "consumed".

Permanent policy refusals deliberately do **not** unmark on either path: a later
copy could only waste work.

#### Double delivery is prevented by MLS, not by bookkeeping

An MLS decrypt consumes the ratchet generation. A copy beaten to it fails.
Reaching the plaintext branch of a drain therefore **proves** first delivery.

An "already delivered elsewhere?" check in that branch is unreachable when true
and a false positive otherwise. The false positive is fatal: the relay path
marks its identifier at arrival, pre-decrypt, so such a check suppresses the
only decryptable copy. That is silent loss.

The drain may keep a set of identifiers delivered **in this drain batch**, but
only to drop a sibling copy without burning a doomed decrypt, and to stop that
sibling's expiry from releasing replay protection for an identifier that was
delivered.
