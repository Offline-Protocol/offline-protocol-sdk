/*
 * Config scenario JSON for offline_protocol_create (G4).
 */

#include "config_scenarios.h"

#include <stdbool.h>
#include <stdio.h>
#include <string.h>

static size_t json_escape(const char *in, char *out, size_t out_len)
{
  size_t need = 3; /* quotes and NUL */
  if (in == NULL)
  {
    return 0;
  }
  for (const unsigned char *p = (const unsigned char *)in; *p != '\0'; p++)
  {
    switch (*p)
    {
    case '"':
    case '\\':
      need += 2;
      break;
    case '\b':
    case '\f':
    case '\n':
    case '\r':
    case '\t':
      need += 2;
      break;
    default:
      if (*p < 0x20)
      {
        need += 6;
      }
      else
      {
        need += 1;
      }
      break;
    }
  }
  if (out == NULL || out_len < need)
  {
    return need;
  }

  size_t w = 0;
  out[w++] = '"';
  for (const unsigned char *p = (const unsigned char *)in; *p != '\0'; p++)
  {
    switch (*p)
    {
    case '"':
      out[w++] = '\\';
      out[w++] = '"';
      break;
    case '\\':
      out[w++] = '\\';
      out[w++] = '\\';
      break;
    case '\b':
      out[w++] = '\\';
      out[w++] = 'b';
      break;
    case '\f':
      out[w++] = '\\';
      out[w++] = 'f';
      break;
    case '\n':
      out[w++] = '\\';
      out[w++] = 'n';
      break;
    case '\r':
      out[w++] = '\\';
      out[w++] = 'r';
      break;
    case '\t':
      out[w++] = '\\';
      out[w++] = 't';
      break;
    default:
      if (*p < 0x20)
      {
        w += (size_t)snprintf(out + w, out_len - w, "\\u%04x", (unsigned)*p);
      }
      else
      {
        out[w++] = (char)*p;
      }
      break;
    }
  }
  out[w++] = '"';
  out[w] = '\0';
  return w;
}

int op_config_scenario_json(
    char *buf,
    size_t buf_len,
    OpConfigScenario scenario,
    const char *app_id,
    const char *profile)
{
  if (buf == NULL || buf_len == 0)
  {
    return -1;
  }
  if (app_id == NULL || profile == NULL)
  {
    return -2;
  }

  char app_esc[512];
  char prof_esc[512];
  if (json_escape(app_id, app_esc, sizeof app_esc) >= sizeof app_esc ||
      json_escape(profile, prof_esc, sizeof prof_esc) >= sizeof prof_esc)
  {
    return -1;
  }

  bool wifi_direct = false;
  bool internet = false;
  bool nostr = false;
  bool prefer_online = false;
  bool encryption_enabled = true;
  bool auto_key_exchange = true;
  bool store_pending = true;
  bool require_encryption_written = false;

  switch (scenario)
  {
  case OpConfigScenario_LocalBle:
    break;
  case OpConfigScenario_ProximityMesh:
    wifi_direct = true;
    break;
  case OpConfigScenario_InternetRelay:
    internet = true;
    prefer_online = true;
    break;
  case OpConfigScenario_NostrMesh:
    nostr = true;
    break;
  case OpConfigScenario_DevelopmentCleartext:
    encryption_enabled = false;
    auto_key_exchange = false;
    store_pending = false;
    require_encryption_written = true;
    break;
  }

  int n = snprintf(
      buf,
      buf_len,
      "{"
      "\"app_id\":%s,"
      "\"profile\":%s,"
      "\"ble_enabled\":true,"
      "\"wifi_direct_enabled\":%s,"
      "\"internet_enabled\":%s,"
      "\"reticulum_enabled\":false,"
      "\"nostr_enabled\":%s,"
      "\"prefer_online\":%s,"
      "\"initial_ttl\":8,"
      "\"encryption_enabled\":%s,"
      "\"auto_key_exchange\":%s,"
      "\"store_pending\":%s,"
      "%s"
      "\"max_pending_per_peer\":64,"
      "\"max_pending_global\":4096,"
      "\"pending_ttl_ms\":86400000,"
      "\"overflow_policy\":\"DropOldest\""
      "}",
      app_esc,
      prof_esc,
      wifi_direct ? "true" : "false",
      internet ? "true" : "false",
      nostr ? "true" : "false",
      prefer_online ? "true" : "false",
      encryption_enabled ? "true" : "false",
      auto_key_exchange ? "true" : "false",
      store_pending ? "true" : "false",
      require_encryption_written ? "\"require_encryption\":false," : "");

  if (n < 0 || (size_t)n >= buf_len)
  {
    return -1;
  }
  return 0;
}
