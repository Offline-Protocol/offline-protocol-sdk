/*
 * Opinionated create()-config presets for C hosts (G4).
 *
 * Hand-written; not generated. Mirrors bindings/react-native/src/configScenarios.ts.
 * Scenario wire ids: localBle, proximityMesh, internetRelay, nostrMesh,
 * developmentCleartext.
 *
 * Copyright (c) 2025-2026 Offline Protocol, Inc.
 * SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)
 */

#ifndef OFFLINE_PROTOCOL_CONFIG_SCENARIOS_H
#define OFFLINE_PROTOCOL_CONFIG_SCENARIOS_H

#include <stddef.h>

typedef enum OpConfigScenario {
  OpConfigScenario_LocalBle,
  OpConfigScenario_ProximityMesh,
  OpConfigScenario_InternetRelay,
  OpConfigScenario_NostrMesh,
  OpConfigScenario_DevelopmentCleartext,
} OpConfigScenario;

/**
 * Writes JSON suitable for offline_protocol_create.
 *
 * @return 0 on success, -1 if buf is NULL or too small, -2 if app_id or profile is NULL.
 */
int op_config_scenario_json(
    char *buf,
    size_t buf_len,
    OpConfigScenario scenario,
    const char *app_id,
    const char *profile);

#endif /* OFFLINE_PROTOCOL_CONFIG_SCENARIOS_H */
