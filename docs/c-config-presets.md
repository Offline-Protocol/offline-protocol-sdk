# C config presets (`op_config_scenario_json`)

G4 PR5 (C half): hand-written helper for hosts on
[the C API](../bindings/c/README.md). **No UDL or wire changes.** Mirrors
[React Native `forScenario`](react-native-config-presets.md).

## Usage

```c
#include "config_scenarios.h"
#include "offline_protocol.h"

char config[4096];
if (op_config_scenario_json(config, sizeof config,
        OpConfigScenario_LocalBle, "com.example.app", "default") != 0) {
  /* buffer too small or bad args */
}

OfflineProtocolHandle *engine = NULL;
OfflineProtocolError err = {0};
if (offline_protocol_create(config, &engine, &err) != OfflineProtocolStatus_Ok) {
  /* ... */
}
```

Compile `bindings/c/src/config_scenarios.c` alongside your host. The header is
`bindings/c/include/config_scenarios.h`.

## Scenario enum

| `OpConfigScenario` | Wire id (TS/RN) |
|--------------------|-----------------|
| `OpConfigScenario_LocalBle` | `localBle` |
| `OpConfigScenario_ProximityMesh` | `proximityMesh` |
| `OpConfigScenario_InternetRelay` | `internetRelay` |
| `OpConfigScenario_NostrMesh` | `nostrMesh` |
| `OpConfigScenario_DevelopmentCleartext` | `developmentCleartext` |

`developmentCleartext` is lab-only; production hosts should omit it.

## Related

- [bindings/c/README.md](../bindings/c/README.md)
- [docs/bridges/c.md](bridges/c.md)
