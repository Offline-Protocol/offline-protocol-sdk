/*
 * Two engines in one process, with the host in between: what a C host owes
 * the engine (docs/bridges/c.md, "What the host owes"), done in C.
 *
 * Each engine has its own file stores and only the stream transport. The
 * host announces each engine's address to the other, as a stream preamble
 * would, then moves frames: it drains one engine's outbound queue with
 * `wifi_direct_get_next_message` and hands each body to the other with
 * `wifi_direct_message_received`, the bytes crossing as base64 in JSON both
 * ways. The engines form an MLS session over that by themselves, and a
 * message goes each way.
 *
 * What this proves that main.c cannot: a transport notification that fires,
 * bytes that survive the JSON round trip in both directions, and an
 * encrypted exchange driven by a C host and nothing else.
 *
 * Exits nonzero on the first expectation that does not hold.
 */
#define _XOPEN_SOURCE 700
#define _DARWIN_C_SOURCE

#include <ftw.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "offline_protocol.h"

/* Rounds of the pump before giving up. A session forms in a handful; the
 * margin is for a loaded runner, and a round with nothing to move sleeps. */
#define MAX_ROUNDS 600

static void die(const char *what, const OfflineProtocolError *err) {
  fprintf(stderr, "FAIL %s: %s %s\n", what, err && err->variant ? err->variant : "",
          err && err->message ? err->message : "");
  exit(1);
}

/* A call that must succeed; returns the result, which the caller frees. */
static char *must(const OfflineProtocolHandle *h, const char *method, const char *params) {
  OfflineProtocolError err;
  char *out = NULL;
  memset(&err, 0, sizeof err);
  if (offline_protocol_call(h, method, params, &out, &err) != OfflineProtocolStatus_Ok) {
    die(method, &err);
  }
  return out;
}

static void must_void(const OfflineProtocolHandle *h, const char *method, const char *params) {
  offline_protocol_string_free(must(h, method, params));
}

/* The string value of `"key":"..."` in `json`, copied, or NULL. Enough for
 * the library's own output: addresses and base64 never contain an escape. */
static char *field(const char *json, const char *key) {
  char pattern[64];
  const char *start, *end;
  char *copy;
  snprintf(pattern, sizeof pattern, "\"%s\":\"", key);
  start = strstr(json, pattern);
  if (start == NULL) return NULL;
  start += strlen(pattern);
  end = strchr(start, '"');
  if (end == NULL) return NULL;
  copy = malloc((size_t)(end - start) + 1);
  if (copy == NULL) return NULL;
  memcpy(copy, start, (size_t)(end - start));
  copy[end - start] = '\0';
  return copy;
}

/* "\"off1...\"" -> "off1...". */
static char *unquote(char *json_string) {
  size_t len = strlen(json_string);
  char *copy = malloc(len);
  if (copy == NULL || len < 2) exit(1);
  memcpy(copy, json_string + 1, len - 2);
  copy[len - 2] = '\0';
  offline_protocol_string_free(json_string);
  return copy;
}

struct device {
  const char *name;
  OfflineProtocolHandle *h;
  char *address;
  char mls[64];
  char state[64];
  int notified;   /* outbound notifications from the stream transport */
  int dropped;    /* the notification's ctx, released by the library */
  int received;   /* message_received events carrying the expected text */
  int moved;      /* frames the host carried out of this engine */
};

static void on_outbound(void *ctx) { ((struct device *)ctx)->notified++; }
static void on_drop(void *ctx) { ((struct device *)ctx)->dropped++; }

static const char *CONFIG_FORMAT =
    "{\"app_id\":\"com.example.c\",\"profile\":\"%s\","
    "\"ble_enabled\":false,\"wifi_direct_enabled\":true,\"internet_enabled\":false,"
    "\"reticulum_enabled\":false,\"nostr_enabled\":false,\"prefer_online\":false,"
    "\"initial_ttl\":8,\"encryption_enabled\":true,\"auto_key_exchange\":true,"
    "\"require_encryption\":true,"
    "\"store_pending\":true,\"max_pending_per_peer\":64,\"max_pending_global\":4096,"
    "\"pending_ttl_ms\":60000,\"overflow_policy\":\"DropOldest\"}";

static void bring_up(struct device *d) {
  OfflineProtocolError err;
  char config[1024], params[512];
  memset(&err, 0, sizeof err);
  snprintf(d->mls, sizeof d->mls, "/tmp/op-c-x-%s-mls-XXXXXX", d->name);
  snprintf(d->state, sizeof d->state, "/tmp/op-c-x-%s-state-XXXXXX", d->name);
  if (mkdtemp(d->mls) == NULL || mkdtemp(d->state) == NULL) die("mkdtemp", NULL);

  snprintf(config, sizeof config, CONFIG_FORMAT, d->name);
  if (offline_protocol_create(config, &d->h, &err) != OfflineProtocolStatus_Ok) die("create", &err);
  if (offline_protocol_set_transport_callback(d->h, OfflineProtocolTransport_WifiDirect, on_outbound, on_drop, d,
                                              &err) != OfflineProtocolStatus_Ok) {
    die("set_transport_callback", &err);
  }
  snprintf(params, sizeof params,
           "{\"mls_root\":\"%s\",\"state_root\":\"%s\","
           "\"store_key\":\"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=\"}",
           d->mls, d->state);
  must_void(d->h, "initialize_mls_with_file_stores", params);
  d->address = unquote(must(d->h, "local_address", NULL));
  must_void(d->h, "start", NULL);
  must_void(d->h, "wifi_direct_status_changed", "{\"is_connected\":true}");
}

/* The host's side of a stream that proved `peer`'s address. */
static void announce(struct device *d, const struct device *peer) {
  char params[256];
  snprintf(params, sizeof params, "{\"peer_id\":\"%s\"}", peer->address);
  must_void(d->h, "wifi_direct_peer_connected", params);
}

/* Moves every queued frame out of `from` into `to`. Returns how many. */
static int carry(struct device *from, struct device *to) {
  int moved = 0;
  for (;;) {
    char *out = must(from->h, "wifi_direct_get_next_message", NULL);
    char *recipient, *data, *params;
    size_t size;
    if (strcmp(out, "null") == 0) {
      offline_protocol_string_free(out);
      return moved;
    }
    recipient = field(out, "recipient_id");
    data = field(out, "data");
    offline_protocol_string_free(out);
    if (recipient == NULL || data == NULL) die("an outbound frame without recipient_id or data", NULL);
    if (strcmp(recipient, to->address) != 0) {
      fprintf(stderr, "FAIL %s queued a frame for %s, not %s\n", from->name, recipient, to->address);
      exit(1);
    }
    size = strlen(from->address) + strlen(data) + 64;
    params = malloc(size);
    if (params == NULL) exit(1);
    snprintf(params, size, "{\"sender_id\":\"%s\",\"data\":\"%s\"}", from->address, data);
    must_void(to->h, "wifi_direct_message_received", params);
    free(params);
    free(recipient);
    free(data);
    moved++;
    from->moved++;
  }
}

/* Drains `d`'s events, counting deliveries of `text`. */
static void drain_events(struct device *d, const char *text) {
  char *event;
  char needle[128];
  snprintf(needle, sizeof needle, "\"content\":\"%s\"", text);
  while ((event = offline_protocol_poll_event(d->h)) != NULL) {
    if (strstr(event, "\"type\":\"message_received\"") != NULL && strstr(event, needle) != NULL) {
      d->received++;
    }
    offline_protocol_string_free(event);
  }
}

/* Pumps both ways, ticking both engines, until `to` has `text`. */
static void deliver(struct device *from, struct device *to, const char *text) {
  char params[256];
  int round;
  snprintf(params, sizeof params, "{\"recipient\":\"%s\",\"content\":\"%s\",\"priority\":\"Medium\"}", to->address,
           text);
  must_void(from->h, "send_message", params);
  for (round = 0; round < MAX_ROUNDS && to->received == 0; round++) {
    int moved = carry(from, to) + carry(to, from);
    must_void(from->h, "process", NULL);
    must_void(to->h, "process", NULL);
    drain_events(to, text);
    drain_events(from, "\x01"); /* keep the sender's queue short; counts nothing */
    if (moved == 0) {
      struct timespec pause = {0, 10 * 1000 * 1000}; /* usleep left POSIX in 2008 */
      nanosleep(&pause, NULL);
    }
  }
  if (to->received != 1) {
    fprintf(stderr, "FAIL %s -> %s: \"%s\" delivered %d times after %d rounds\n", from->name, to->name, text,
            to->received, round);
    exit(1);
  }
}

static int remove_one(const char *path, const struct stat *sb, int flag, struct FTW *ftw) {
  (void)sb;
  (void)flag;
  (void)ftw;
  return remove(path);
}

static void bring_down(struct device *d) {
  must_void(d->h, "stop", NULL);
  must_void(d->h, "close_file_stores", NULL);
  offline_protocol_free(d->h);
  d->h = NULL;
  nftw(d->mls, remove_one, 8, FTW_DEPTH | FTW_PHYS);
  nftw(d->state, remove_one, 8, FTW_DEPTH | FTW_PHYS);
  free(d->address);
}

int main(void) {
  struct device a, b;
  memset(&a, 0, sizeof a);
  memset(&b, 0, sizeof b);
  a.name = "alice";
  b.name = "bob";

  bring_up(&a);
  bring_up(&b);
  if (strcmp(a.address, b.address) == 0) die("two engines share an address", NULL);
  announce(&a, &b);
  announce(&b, &a);

  deliver(&a, &b, "hello from alice");
  deliver(&b, &a, "hello from bob");

  if (a.notified == 0 || b.notified == 0) {
    fprintf(stderr, "FAIL the stream transport never notified (alice %d, bob %d)\n", a.notified, b.notified);
    return 1;
  }
  if (a.moved == 0 || b.moved == 0) die("a direction carried nothing", NULL);

  bring_down(&a);
  bring_down(&b);
  if (a.dropped != 1 || b.dropped != 1) {
    fprintf(stderr, "FAIL transport ctx dropped alice %d, bob %d times\n", a.dropped, b.dropped);
    return 1;
  }
  printf("c exchange check: ok (frames alice->bob %d, bob->alice %d; notifications %d, %d)\n", a.moved, b.moved,
         a.notified, b.notified);
  return 0;
}
