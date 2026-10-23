# The Offline Protocol SDK from Ruby

A typed Ruby API for scripts, tooling and servers, generated from the same
interface definition as the Swift, Kotlin, Python and Java bindings by UniFFI's
own Ruby generator. It calls the native library directly through the
[`ffi`](https://github.com/ffi/ffi) gem: there is no native glue of its own and
it does not go through the C API.

The whole binding is one file, [`lib/offline_protocol.rb`](lib/offline_protocol.rb).
The module is `OfflineProtocol` and the engine inside it is
`OfflineProtocol::OfflineProtocol`. `MeshServices` and `DataStore` hang off the
engine, every record and enum of the definition is a class, and the things a
host implements are plain objects that respond to the interface's methods. An
engine refusal is an exception named for the variant
(`OfflineProtocol::ProtocolError::EncryptFailed`).

The contract is [docs/bridges/ruby.md](../../docs/bridges/ruby.md), on top of
the [shared one](../../docs/bridges/README.md). Read it before writing a
callback or a storage provider.

## What you need

| What | Where it comes from |
|---|---|
| Ruby 3.3 or later and the `ffi` gem, `~> 1.17` | Yours; `bundle install` in this directory installs the gem |
| This binding | The [`offline_protocol`](https://rubygems.org/gems/offline_protocol) gem once a release publishes it (R7), or [`lib/offline_protocol.rb`](lib/offline_protocol.rb) on your load path |
| `liboffline_protocol_uniffi.so` / `.dylib`, `offline_protocol_uniffi.dll` | The release's desktop archives, or `cargo build --release -p offline-protocol-uniffi` |

The library is found on the loader's search path, and the loader reads the
variable when the process starts, so set it before Ruby launches:

```sh
# Linux
LD_LIBRARY_PATH=/path/to/dir ruby -Ilib your_script.rb
# macOS
DYLD_LIBRARY_PATH=/path/to/dir ruby -Ilib your_script.rb
# Windows (RubyInstaller reads RUBY_DLL_PATH, not PATH)
$env:RUBY_DLL_PATH = "C:\path\to\dir"; ruby -Ilib your_script.rb
```

A protected system Ruby on macOS (`/usr/bin/ruby`) drops `DYLD_LIBRARY_PATH`;
use a Ruby you installed.

## Quick start

```ruby
require "offline_protocol"

Sdk = OfflineProtocol

# An instance-less function needs no engine.
puts Sdk.derive_address([0] * 32)

# Name the fields the definition leaves without a default; the rest take it.
config = Sdk::ProtocolConfig.new(
  app_id: "com.example.ruby", profile: "ruby-example",
  ble_enabled: false, wifi_direct_enabled: false, internet_enabled: true,
  reticulum_enabled: false, nostr_enabled: false, prefer_online: false,
  initial_ttl: 8, encryption_enabled: true, auto_key_exchange: true,
  # The pending queue: per peer, global, the SDK's 24 h lifetime, overflow.
  store_pending: true, max_pending_per_peer: 64, max_pending_global: 4096,
  pending_ttl_ms: 86_400_000, overflow_policy: Sdk::OverflowPolicy::DROP_OLDEST
)

engine = Sdk::OfflineProtocol.new(config)

# Any object that responds to on_event. It runs inside the engine call.
listener = Object.new
def listener.on_event(json) = puts(json)
engine.set_event_callback(listener)

engine.emit_test_event
```

Keep the engine reachable for as long as the process runs. See R3 in the
contract for what a collected engine costs.

A byte array the engine returns comes back signed, -128 to 127, and has to be
masked with `map { |b| b & 0xFF }` before it goes back in (R8).

A complete, runnable program is in [examples/ruby-app](../../examples/ruby-app/README.md).

## Storage

The engine needs somewhere to keep key material and protocol state. Either hand
it the built-in file stores:

```ruby
engine.initialize_mls_with_file_stores(mls_dir, state_dir, store_key_32_bytes)
```

or write two providers and pass them to `initialize_mls`. They carry bytes
differently (R5 in the contract), and a provider you write should pass
`OfflineProtocol.run_storage_conformance`. [`test/memory_storage.rb`](test/memory_storage.rb)
is a complete in-memory pair.

## Tests

```sh
cargo build -p offline-protocol-uniffi
cd bindings/ruby
bundle install
bundle exec rake test
```

`rake test` puts `../../target/debug` on the loader's search path (set
`OFFLINE_PROTOCOL_LIBRARY_DIR` for another build) and runs
[`test/consumer_check_test.rb`](test/consumer_check_test.rb) against the real
library. The generated code does not compare the library's FFI checksums, so
these tests are what show that the binding and the library came from the same
definition. Outside this repository nothing checks it: take the library from
the same release as the gem (R1).

`bundle exec rake build` writes the gem to `pkg/`, at the version in
`OFFLINE_PROTOCOL_VERSION` (default `0.0.0`). Use the task, not `gem build`: it
stages the license notices from the repository root, which the gemspec names
and `gem build` cannot reach.

## Regenerating

Never edit `lib/offline_protocol.rb`. It is regenerated with the other four
bindings, which are one artifact set:

```sh
./scripts/generate-bindings.sh
```

The Ruby generator is part of `uniffi-bindgen`, so there is nothing more to
install than the other bindings need. [`uniffi.toml`](uniffi.toml) names the
library; see R1 in the contract.
