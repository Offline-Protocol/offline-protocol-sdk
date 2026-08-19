# Storage adapter references

Working `ProtocolStateStorageProvider` implementations, one per binding,
each backed by SQLite and each wired to the conformance suite.

They exist to prove a property rather than to be dropped into an app: the SDK
ships a working provider on every platform, so an application only needs one
of these when it already has a store it wants its data inside.

| File | Binding |
|---|---|
| [`swift/SqliteProtocolStateStorage.swift`](swift/SqliteProtocolStateStorage.swift) | Swift, SQLite3 C API |
| [`kotlin/SqliteProtocolStateStorage.kt`](kotlin/SqliteProtocolStateStorage.kt) | Kotlin, `android.database.sqlite` |
| [`python/sqlite_state_storage.py`](python/sqlite_state_storage.py) | Python, stdlib `sqlite3` |

Rust consumers assign the trait object directly:

```rust
let config = ProtocolConfig::builder("my-app", "default")
    .data_enabled(true)
    .data_storage(Arc::new(MyBackend::open("documents.db")?))
    .build()?;
```

## The contract

Four methods over `(key_type, key_id, bytes)`, and one gate:

```
runStorageConformance(provider) -> {"passed": [...], "failures": [...]}
```

Green means supported. Each adapter above ships a test that runs it.

Three things carry the most weight, and each is a check in the suite because
the failure is otherwise invisible:

- **Bytes, not text.** Sealed records are ciphertext. A backend that passes
  values through a string type corrupts them, and the symptom is a record
  that will not open, much later.
- **Store replaces.** A second write under the same key must overwrite. A
  backend that keeps the first value serves stale data indefinitely.
- **Absent is not an error.** A key that was never written loads as "no
  value"; the SDK asks for records that legitimately do not exist yet on
  every launch. Deleting an absent key is likewise fine, because the data
  layer removes folded delta records a crash may already have taken.

## Sealing, and what an adapter never sees

Sealing sits above this seam. Records whose category requires it are
encrypted inside the core before `store` is called and decrypted after
`load` returns, so an adapter is handed sealed bytes and never sees document
content or message plaintext. Swapping backends cannot change the at-rest
posture, which is the point: the security property does not depend on the
adapter author's care.

## Logout

`wipePersistedState()` clears the account directory of the **default**
provider. A custom backend is not inside it, so an application that
configures one must call `DataStore.wipeAll()` on logout. Skipping it leaves
documents behind after the account that made them is gone.

See [C11 in the bridge contracts](../../docs/bridges/README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified).
