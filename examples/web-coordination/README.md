# Device coordination

Every tab is a device that publishes its own live state and subscribes to every other device it finds, with no server. It shows service subscriptions ([ADR 0035](../../docs/adr/0035-a-service-can-be-subscribed-to.md)) end to end in the browser.

```bash
cargo install wasm-bindgen-cli --version 0.2.105 --locked   # once; must match Cargo.lock
bash bindings/web/scripts/build.sh
python3 -m http.server 8000 --bind 127.0.0.1                 # from the repository root
```

Open <http://127.0.0.1:8000/examples/web-coordination/> in two or more tabs. The tabs find each other over a `BroadcastChannel` and form MLS sessions on their own.

What each tab does:

- **Registers `coord.v1`** and publishes to two topics on it. `status` is a `latest` topic: the name, status and load sliders each publish one key, and only the newest value per key matters. `alerts` is a `log` topic: every alert is delivered, in order, and a checkpoint summarises the ones a late subscriber can no longer be replayed.
- **Subscribes to both topics on every neighbour**, as soon as it is discovered. A subscription made before the session exists waits for it.
- **Shows what each publish reached**: subscribers sent the update live, queued for them, or held back by the 50 ms update interval (drag the load slider to see coalescing).
- **Who may watch** switches the service's policy between admitting every peer with a session and asking you. Under *ask me*, a new subscriber (open another tab) waits under *Waiting for your answer*.
- **Resync** asks a device for its whole state again.

Every subscription frame is sealed in the 1:1 session between the two tabs; there is no plaintext form. Close a tab and its subscriptions on the others pause, then end after ten minutes without an answer.
