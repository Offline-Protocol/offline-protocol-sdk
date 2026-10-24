# Flutter plugin size baselines (contributors)

CI compares the release Android AAR against committed size baselines. App developers can ignore this directory.

| File | What it measures |
|------|------------------|
| `offline-protocol-release-aar.json` | `:offline_protocol:assembleRelease` AAR |

Update after an intentional native surface change:

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh --write-baseline
```

Verify locally (same as CI):

```bash
bindings/flutter/scripts/measure-plugin-aar-size.sh --check
```
