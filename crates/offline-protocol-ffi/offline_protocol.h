#ifndef OFFLINE_PROTOCOL_H
#define OFFLINE_PROTOCOL_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Error codes for FFI operations
 */
typedef enum ErrorCode {
  Success = 0,
  InvalidArgument = 1,
  NotStarted = 2,
  AlreadyStarted = 3,
  SendFailed = 4,
  PermissionDenied = 5,
  Unknown = 99,
} ErrorCode;

typedef struct OfflineProtocolHandle OfflineProtocolHandle;

/**
 * Create a new OfflineProtocol instance
 *
 * # Safety
 * - `config_json` must be a valid null-terminated UTF-8 string
 * - The returned handle must be freed with `offline_protocol_free`
 */
struct OfflineProtocolHandle *offline_protocol_new(const char *config_json);

/**
 * Free an OfflineProtocol instance
 *
 * # Safety
 * - `handle` must be a valid pointer returned from `offline_protocol_new`
 * - `handle` must not be used after this call
 */
void offline_protocol_free(struct OfflineProtocolHandle *handle);

/**
 * Start the protocol
 *
 * # Safety
 * - `handle` must be a valid pointer returned from `offline_protocol_new`
 */
enum ErrorCode offline_protocol_start(struct OfflineProtocolHandle *handle);

/**
 * Stop the protocol
 *
 * # Safety
 * - `handle` must be a valid pointer returned from `offline_protocol_new`
 */
enum ErrorCode offline_protocol_stop(struct OfflineProtocolHandle *handle);

/**
 * Send a message
 *
 * # Safety
 * - `handle` must be a valid pointer
 * - `message_json` must be a valid null-terminated UTF-8 string
 * - Returns a message ID as a JSON string (caller must free with `offline_protocol_free_string`)
 */
char *offline_protocol_send_message(struct OfflineProtocolHandle *handle, const char *message_json);

/**
 * Free a string returned by the FFI
 *
 * # Safety
 * - `s` must be a string returned from an FFI function
 */
void offline_protocol_free_string(char *s);

/**
 * Get the library version
 *
 * # Safety
 * - Returns a static string, no need to free
 */
const char *offline_protocol_version(void);

#endif  /* OFFLINE_PROTOCOL_H */
