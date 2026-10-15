/*
 * A C program against the SDK's C API: what CI compiles and runs to prove
 * the header, the library and the contract in docs/bridges/c.md agree when
 * a C compiler, not Rust, is on the other side.
 *
 * Exits nonzero on the first expectation that does not hold. Built by the
 * Makefile beside it with -std=c99 -Wall -Wextra -Wpedantic -Werror, once
 * against the shared library and once against the static one.
 */
/* mkdtemp and strdup: POSIX 2008, which glibc and macOS each gate their own way. */
#define _XOPEN_SOURCE 700
#define _DARWIN_C_SOURCE

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "offline_protocol.h"

static int failures = 0;

#define EXPECT(cond, ...)                                    \
  do {                                                       \
    if (!(cond)) {                                           \
      fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__);   \
      fprintf(stderr, __VA_ARGS__);                          \
      fputc('\n', stderr);                                   \
      failures++;                                            \
    }                                                        \
  } while (0)

static int starts_with(const char *s, const char *prefix) {
  return s != NULL && strncmp(s, prefix, strlen(prefix)) == 0;
}

/* One call through offline_protocol_call; prints and clears any error. */
static OfflineProtocolStatus call(const OfflineProtocolHandle *h, const char *method,
                                  const char *params, char **out, OfflineProtocolError *err) {
  OfflineProtocolStatus status;
  *out = NULL;
  status = offline_protocol_call(h, method, params, out, err);
  return status;
}

/* ---- events -------------------------------------------------------------- */

struct seen {
  int events;
  int drops;
  char *last;
  pthread_t thread;
};

static void on_event(void *ctx, const char *json) {
  struct seen *seen = (struct seen *)ctx;
  seen->events++;
  free(seen->last);
  seen->last = strdup(json);
  seen->thread = pthread_self();
}

static void on_drop(void *ctx) { ((struct seen *)ctx)->drops++; }

/* ---- a storage provider, as a C host writes one ----------------------------- */

struct entry {
  char *type;
  char *id;
  uint8_t *data;
  size_t len;
  struct entry *next;
};

struct store {
  pthread_mutex_t lock;
  struct entry *head;
};

static struct entry **find(struct store *s, const char *type, const char *id) {
  struct entry **at = &s->head;
  while (*at != NULL && !(strcmp((*at)->type, type) == 0 && strcmp((*at)->id, id) == 0)) {
    at = &(*at)->next;
  }
  return at;
}

static int32_t s_store(void *ctx, const char *type, const char *id, const uint8_t *data, size_t len) {
  struct store *s = (struct store *)ctx;
  struct entry **at;
  uint8_t *copy = malloc(len > 0 ? len : 1);
  if (copy == NULL) return OfflineProtocolStorageStatus_StoreFailed;
  if (len > 0) memcpy(copy, data, len);
  pthread_mutex_lock(&s->lock);
  at = find(s, type, id);
  if (*at == NULL) {
    *at = calloc(1, sizeof(struct entry));
    if (*at == NULL) {
      pthread_mutex_unlock(&s->lock);
      free(copy);
      return -1; /* outside the range: read as StoreFailed, never as corruption */
    }
    (*at)->type = strdup(type);
    (*at)->id = strdup(id);
  } else {
    free((*at)->data);
  }
  (*at)->data = copy;
  (*at)->len = len;
  pthread_mutex_unlock(&s->lock);
  return OfflineProtocolStorageStatus_Ok;
}

static int32_t s_load(void *ctx, const char *type, const char *id, uint8_t **out, size_t *out_len) {
  struct store *s = (struct store *)ctx;
  struct entry *e;
  pthread_mutex_lock(&s->lock);
  e = *find(s, type, id);
  if (e != NULL) {
    *out = malloc(e->len > 0 ? e->len : 1);
    if (*out == NULL) {
      pthread_mutex_unlock(&s->lock);
      return OfflineProtocolStorageStatus_LoadFailed;
    }
    if (e->len > 0) memcpy(*out, e->data, e->len);
    *out_len = e->len;
  }
  pthread_mutex_unlock(&s->lock);
  return OfflineProtocolStorageStatus_Ok;
}

static int32_t s_remove(void *ctx, const char *type, const char *id) {
  struct store *s = (struct store *)ctx;
  struct entry **at, *gone;
  pthread_mutex_lock(&s->lock);
  at = find(s, type, id);
  gone = *at;
  if (gone != NULL) {
    *at = gone->next;
    free(gone->type);
    free(gone->id);
    free(gone->data);
    free(gone);
  }
  pthread_mutex_unlock(&s->lock);
  return OfflineProtocolStorageStatus_Ok;
}

/* Key ids are the library's own (hex, base64url, fixed names), never ones
 * that need escaping, so a JSON array is written directly. */
static int32_t s_list(void *ctx, const char *type, uint8_t **out, size_t *out_len) {
  struct store *s = (struct store *)ctx;
  struct entry *e;
  size_t cap = 2, used = 0;
  char *json;
  pthread_mutex_lock(&s->lock);
  for (e = s->head; e != NULL; e = e->next) {
    if (strcmp(e->type, type) == 0) cap += strlen(e->id) + 3;
  }
  json = malloc(cap + 1);
  if (json == NULL) {
    pthread_mutex_unlock(&s->lock);
    return OfflineProtocolStorageStatus_LoadFailed;
  }
  json[used++] = '[';
  for (e = s->head; e != NULL; e = e->next) {
    if (strcmp(e->type, type) != 0) continue;
    if (used > 1) json[used++] = ',';
    used += (size_t)sprintf(json + used, "\"%s\"", e->id);
  }
  json[used++] = ']';
  pthread_mutex_unlock(&s->lock);
  *out = (uint8_t *)json;
  *out_len = used;
  return OfflineProtocolStorageStatus_Ok;
}

static void s_release(void *ctx, uint8_t *data, size_t len) {
  (void)ctx;
  (void)len;
  free(data);
}

static OfflineProtocolStorage provider(struct store *s) {
  OfflineProtocolStorage p;
  memset(&p, 0, sizeof p);
  p.ctx = s;
  p.store = s_store;
  p.load = s_load;
  p.remove = s_remove;
  p.list_keys = s_list;
  p.release = s_release;
  p.drop = NULL;
  return p;
}

/* ---- the run --------------------------------------------------------------- */

static const char *CONFIG =
    "{\"app_id\":\"com.example.c\",\"profile\":\"c-consumer\","
    "\"ble_enabled\":false,\"wifi_direct_enabled\":false,\"internet_enabled\":true,"
    "\"reticulum_enabled\":false,\"nostr_enabled\":false,\"prefer_online\":false,"
    "\"initial_ttl\":8,\"encryption_enabled\":true,\"auto_key_exchange\":true,"
    "\"store_pending\":true,\"max_pending_per_peer\":64,\"max_pending_global\":4096,"
    "\"pending_ttl_ms\":60000,\"overflow_policy\":\"DropOldest\"}";

int main(void) {
  OfflineProtocolError err;
  OfflineProtocolHandle *h = NULL;
  OfflineProtocolStatus status;
  char *out = NULL;
  struct seen seen;
  struct store secure, state;
  OfflineProtocolStorage secure_p, state_p;
  char mls_dir[] = "/tmp/op-c-mls-XXXXXX";
  char state_dir[] = "/tmp/op-c-state-XXXXXX";
  char params[512];

  memset(&err, 0, sizeof err);
  memset(&seen, 0, sizeof seen);

  /* Version and an instance-less call, before any engine exists. */
  EXPECT(strlen(offline_protocol_version()) > 0, "empty version");
  status = offline_protocol_call_static(
      "derive_address", "{\"public_key\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"}", &out, &err);
  EXPECT(status == OfflineProtocolStatus_Ok, "derive_address: %d", (int)status);
  EXPECT(starts_with(out, "\"off1"), "derive_address gave %s", out ? out : "(null)");
  offline_protocol_string_free(out);

  /* An engine. */
  status = offline_protocol_create(CONFIG, &h, &err);
  EXPECT(status == OfflineProtocolStatus_Ok && h != NULL, "create: %s", err.message ? err.message : "");
  if (h == NULL) return 1;

  EXPECT(call(h, "get_state", NULL, &out, &err) == OfflineProtocolStatus_Ok, "get_state");
  EXPECT(out != NULL && strcmp(out, "\"Stopped\"") == 0, "state %s", out ? out : "(null)");
  offline_protocol_string_free(out);

  /* Events: on the calling thread, and the same text in the poll queue. */
  EXPECT(offline_protocol_set_event_callback(h, on_event, on_drop, &seen, &err) == OfflineProtocolStatus_Ok,
         "set_event_callback");
  EXPECT(call(h, "emit_test_event", NULL, &out, &err) == OfflineProtocolStatus_Ok, "emit_test_event");
  offline_protocol_string_free(out);
  EXPECT(seen.events == 1, "events %d", seen.events);
  EXPECT(seen.events == 0 || pthread_equal(seen.thread, pthread_self()), "event on another thread");
  out = offline_protocol_poll_event(h);
  EXPECT(out != NULL && seen.last != NULL && strcmp(out, seen.last) == 0, "poll gave %s", out ? out : "(null)");
  offline_protocol_string_free(out);
  EXPECT(offline_protocol_poll_event(h) == NULL, "queue not empty");

  /* Refusals, each with its status. */
  EXPECT(call(h, "no_such_method", NULL, &out, &err) == OfflineProtocolStatus_MethodNotFound, "unknown method");
  offline_protocol_error_clear(&err);
  EXPECT(call(h, "get_state", "{", &out, &err) == OfflineProtocolStatus_InvalidArgument, "bad JSON");
  offline_protocol_error_clear(&err);
  EXPECT(call(h, "get_state", "{\"x\":1}", &out, &err) == OfflineProtocolStatus_InvalidParams, "extra key");
  offline_protocol_error_clear(&err);
  EXPECT(offline_protocol_set_transport_callback(h, 7, NULL, NULL, NULL, &err) ==
             OfflineProtocolStatus_InvalidArgument,
         "transport 7");
  offline_protocol_error_clear(&err);

  /* An engine refusal carries the definition's variant and position. */
  EXPECT(call(h, "start", NULL, &out, &err) == OfflineProtocolStatus_Ok, "start");
  offline_protocol_string_free(out);
  status = call(h, "send_message",
                "{\"recipient\":\"off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s\",\"content\":\"hi\","
                "\"priority\":\"Medium\"}",
                &out, &err);
  EXPECT(status == OfflineProtocolStatus_EngineError, "send before storage: %d", (int)status);
  EXPECT(err.variant != NULL && strcmp(err.variant, "EncryptFailed") == 0, "variant %s",
         err.variant ? err.variant : "(null)");
  EXPECT(err.code == 6, "code %d", (int)err.code);
  offline_protocol_error_clear(&err);
  EXPECT(err.variant == NULL && err.message == NULL, "error_clear left strings");
  EXPECT(call(h, "stop", NULL, &out, &err) == OfflineProtocolStatus_Ok, "stop");
  offline_protocol_string_free(out);

  /* A C storage provider passes the conformance suite. */
  memset(&state, 0, sizeof state);
  pthread_mutex_init(&state.lock, NULL);
  state_p = provider(&state);
  EXPECT(offline_protocol_run_storage_conformance(&state_p, &out, &err) == OfflineProtocolStatus_Ok, "conformance");
  EXPECT(out != NULL && strstr(out, "\"failures\":[]") != NULL, "conformance report %s", out ? out : "(null)");
  offline_protocol_string_free(out);

  /* initialize_mls over two C providers gives the engine an address. It
   * belongs before start(), on a fresh engine, as on every binding. */
  offline_protocol_free(h);
  EXPECT(seen.drops == 1, "event ctx dropped %d times", seen.drops);
  free(seen.last);
  h = NULL;
  EXPECT(offline_protocol_create(CONFIG, &h, &err) == OfflineProtocolStatus_Ok, "create for storage");
  if (h == NULL) return 1;
  memset(&secure, 0, sizeof secure);
  pthread_mutex_init(&secure.lock, NULL);
  secure_p = provider(&secure);
  EXPECT(offline_protocol_initialize_mls(h, &secure_p, &state_p, &err) == OfflineProtocolStatus_Ok,
         "initialize_mls: %s", err.message ? err.message : "");
  offline_protocol_error_clear(&err);
  EXPECT(call(h, "local_address", NULL, &out, &err) == OfflineProtocolStatus_Ok, "local_address");
  EXPECT(starts_with(out, "\"off1"), "local_address %s", out ? out : "(null)");
  offline_protocol_string_free(out);
  EXPECT(secure.head != NULL, "nothing reached the secure provider");
  EXPECT(call(h, "start", NULL, &out, &err) == OfflineProtocolStatus_Ok, "start after storage");
  offline_protocol_string_free(out);
  EXPECT(call(h, "stop", NULL, &out, &err) == OfflineProtocolStatus_Ok, "stop");
  offline_protocol_string_free(out);
  offline_protocol_free(h);

  /* The built-in file stores, through offline_protocol_call. */
  EXPECT(mkdtemp(mls_dir) != NULL && mkdtemp(state_dir) != NULL, "mkdtemp");
  h = NULL;
  EXPECT(offline_protocol_create(CONFIG, &h, &err) == OfflineProtocolStatus_Ok, "create 2");
  snprintf(params, sizeof params,
           "{\"mls_root\":\"%s\",\"state_root\":\"%s\","
           "\"store_key\":\"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=\"}",
           mls_dir, state_dir);
  EXPECT(call(h, "initialize_mls_with_file_stores", params, &out, &err) == OfflineProtocolStatus_Ok,
         "file stores: %s", err.message ? err.message : "");
  offline_protocol_string_free(out);
  offline_protocol_error_clear(&err);
  EXPECT(call(h, "local_address", NULL, &out, &err) == OfflineProtocolStatus_Ok, "local_address 2");
  EXPECT(starts_with(out, "\"off1"), "local_address 2 %s", out ? out : "(null)");
  offline_protocol_string_free(out);
  EXPECT(call(h, "close_file_stores", NULL, &out, &err) == OfflineProtocolStatus_Ok, "close_file_stores");
  offline_protocol_string_free(out);
  offline_protocol_free(h);

  if (failures == 0) {
    printf("c consumer check: ok (library %s)\n", offline_protocol_version());
    return 0;
  }
  fprintf(stderr, "c consumer check: %d failure(s)\n", failures);
  return 1;
}
