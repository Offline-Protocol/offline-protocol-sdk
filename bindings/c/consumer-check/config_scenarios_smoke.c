/* Smoke test for bindings/c/src/config_scenarios.c (no engine required). */

#include <assert.h>
#include <stdio.h>
#include <string.h>

#include "config_scenarios.h"

int main(void)
{
  char buf[4096];
  assert(op_config_scenario_json(buf, sizeof buf, OpConfigScenario_LocalBle, "app", "prof") == 0);
  assert(strstr(buf, "\"ble_enabled\":true") != NULL);
  assert(strstr(buf, "\"internet_enabled\":false") != NULL);

  assert(op_config_scenario_json(buf, sizeof buf, OpConfigScenario_InternetRelay, "a", "p") == 0);
  assert(strstr(buf, "\"prefer_online\":true") != NULL);
  assert(strstr(buf, "\"internet_enabled\":true") != NULL);

  assert(op_config_scenario_json(buf, sizeof buf, OpConfigScenario_DevelopmentCleartext, "a", "p") == 0);
  assert(strstr(buf, "\"encryption_enabled\":false") != NULL);

  puts("config scenarios: ok");
  return 0;
}
