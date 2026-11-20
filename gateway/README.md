# Offline Protocol gateway daemon

The reference implementation of the gateway half of
[the gateway contract](../docs/spec/gateway-contract.md). Devices running the
SDK attach to it over local IP with the transport the SDK calls `reticulum`
(the gateway slot, [ADR 0026](../docs/adr/0026-the-backbone-is-a-gateway-property.md)),
and it carries their sealed frames to each other and, through a backbone, to
devices attached to other gateways.

This is a work in progress; the full guide lands with the Reticulum backbone.

```bash
python -m venv .venv
.venv/bin/pip install -e ../bindings/python -e '.[dev]'
.venv/bin/python -m pytest
```
