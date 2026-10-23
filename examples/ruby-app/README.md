# Ruby example

Two devices, Alice and Bob, in one Ruby process: they pair, establish an
end-to-end encrypted session, use conversation signals, transfer a file, and
create an MLS group, using the [Ruby binding](../../bindings/ruby/README.md).

```
alice off1q8f6g4t...
bob   off1q9l2sgn...
  ok: bob and alice are paired
  ok: bob sees alice online
  ok: bob sees alice typing
  ok: bob received "hello bob" (encrypted end to end)
  ok: alice received a read receipt
  ok: alice received a threaded reply
  ok: bob received the original file bytes
  ok: bob joined the MLS group
  ok: bob received the group message
alice transport ["Internet"]
group members ["off1...", "off1..."]
done
```

## What it shows

- Creating an engine from a `ProtocolConfig` that names only the fields the
  definition gives no default: the generated Ruby record keeps the rest, see
  [R2](../../docs/bridges/ruby.md#r2-the-generated-records-keep-the-definitions-defaults).
- The engine's own encrypted file stores (`initialize_mls_with_file_stores`),
  so no storage provider is needed.
- Pairing from events: the app reads `poll_event`, and answers
  `connection_request_received` and `connection_accepted` with the key package
  exchange.
- Masking a key package back to bytes before it goes back into the engine. The
  generated code reads bytes as signed and refuses them on the way in, see
  [R8](../../docs/bridges/ruby.md#r8-bytes-come-back-signed-and-must-be-masked-before-they-go-back-in).
- Presence and typing indicators, followed by a normal encrypted message, a
  read receipt tied to its message ID, and a high-priority rich reply threaded
  to the original message.
- `send_file`, checked byte for byte against the reassembled `file_received`
  event. That event carries the data as base64 for the host to save or use.
- Creating an MLS group, inviting the paired peer, and sending a group message.
- Reading active transports and group membership back from the SDK.
- A transport's job, in a dozen lines: take what an engine wants to send
  (`internet_get_next_message`), hand it to the engine it is addressed to
  (`internet_message_received`), and report an outcome for every frame
  (`internet_confirm_sent` once carried, `internet_send_failed` when there was
  nobody to carry it to). A real app does this over a radio or a socket.
  Everything else here, the pairing, the MLS session and the encryption, is the
  engine's own.

The in-process carrier delivers frames directly between the two engines. It
does not act as an internet relay or record transport metrics; the event checks
show that the SDK processed each feature end to end.

## Run it

You need Ruby 3.3 or later and the native library.

```bash
cargo build -p offline-protocol-uniffi      # from the repository root
cd examples/ruby-app
bundle install
bundle exec rake run                         # OFFLINE_PROTOCOL_LIBRARY_DIR=<dir> for another library
```

The `Gemfile` takes the binding from `bindings/ruby` in this checkout, so the
example runs against the source it documents. In your own project, depend on
the published gem instead:

```ruby
gem "offline_protocol", "~> <version>"
```

`rake run` puts the library's directory on the loader's search path before it
starts Ruby, because the loader reads it only at launch
([R6](../../docs/bridges/ruby.md#r6-requirements-the-platform-imposes)).

## Where to go next

- Callbacks and storage providers run inside the engine, and the two storage
  interfaces carry bytes differently:
  [R5](../../docs/bridges/ruby.md#r5-callbacks-are-duck-typed-and-the-two-storage-providers-take-different-bytes).
- Keep the engine reachable for the life of the process:
  [R3](../../docs/bridges/ruby.md#r3-an-engine-that-is-collected-while-it-holds-a-callback-leaks-the-callback).
- The host fills the telemetry platform:
  [R4](../../docs/bridges/ruby.md#r4-the-host-fills-the-telemetry-platform).
