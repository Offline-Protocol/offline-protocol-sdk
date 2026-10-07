# Calling a service on another device with plain HTTP

Two programs that know nothing of the SDK, talking across two hosts through
the [HTTP front](../../docs/spec/http-front.md):

- `provider.py` is an ordinary HTTP server (standard library only). It
  answers `/now` with the time and the caller's address, and registers itself
  with the front on its own host at start.
- `client.sh` calls it from another host with `curl`.

On each host, run the service with the front, for example from the
[container image](../../bindings/python/docker):

```bash
offline-protocol-service --config config.json --mls-root keys --state-root state \
    --listen 0.0.0.0:7878 --lan --http 127.0.0.1:8080
```

On the providing host:

```bash
python3 provider.py --front http://127.0.0.1:8080 --port 9000
```

On the calling host, with the provider host's address (from its front's
`GET /health`, `local_address`):

```bash
./client.sh off1qx7jj4u8w32ptzysnkadwjzmz9w2nukfmc3ts2ap
{"time": "2026-10-07T12:49:47+00:00", "tz": "utc", "caller": "off1q9kd..."}
```

`caller` is the calling host's address as the engine authenticated it, which
the provider can use to decide who may call it. With an alias file
(`--http-aliases`, `{"aliases": {"bob": "off1..."}}`), `./client.sh bob`
works too. The Python suite runs the provider against two in-process hosts
(`tests/http_front/test_demo.py`).
