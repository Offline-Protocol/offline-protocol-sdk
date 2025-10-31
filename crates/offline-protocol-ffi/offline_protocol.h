#ifndef OFFLINE_PROTOCOL_H
#define OFFLINE_PROTOCOL_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Success code.
 */
#define SUCCESS 0

/**
 * Error: Null pointer passed as argument.
 */
#define ERROR_NULL_POINTER -1

/**
 * Error: Invalid UTF-8 in string parameter.
 */
#define ERROR_INVALID_UTF8 -2

/**
 * Error: Protocol not started.
 */
#define ERROR_NOT_STARTED -3

/**
 * Error: Protocol already started.
 */
#define ERROR_ALREADY_STARTED -4

/**
 * Error: Failed to send message.
 */
#define ERROR_SEND_FAILED -5

/**
 * Error: Invalid configuration.
 */
#define ERROR_INVALID_CONFIG -6

/**
 * Error: Rust panic occurred (bug).
 */
#define ERROR_PANIC -99

/**
 * Error: Other unspecified error.
 */
#define ERROR_OTHER -100

/**
 * Creates a new OfflineProtocol instance from JSON configuration.
 *
 * # Safety
 *
 * `config_json` must be a valid null-terminated C string.
 * Returns a pointer to OfflineProtocol on success, or null on failure.
 */
OfflineProtocol *offline_protocol_create(const char *config_json);

/**
 * Destroys an OfflineProtocol instance and frees its memory.
 *
 * # Safety
 *
 * `handle` must be a valid pointer returned by `offline_protocol_create`.
 * After calling this function, the handle is invalid and must not be used.
 */
void offline_protocol_destroy(OfflineProtocol *handle);

/**
 * Starts the protocol.
 *
 * # Safety
 *
 * `handle` must be a valid pointer returned by `offline_protocol_create`.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_start(OfflineProtocol *handle);

/**
 * Stops the protocol.
 *
 * # Safety
 *
 * `handle` must be a valid pointer returned by `offline_protocol_create`.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_stop(OfflineProtocol *handle);

/**
 * Sends a message.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `recipient` and `content` must be valid null-terminated C strings.
 * - `out_message_id` must point to a buffer of at least `out_len` bytes.
 *
 * # Returns
 *
 * Returns SUCCESS and writes message ID to `out_message_id`, or an error code.
 */
int32_t offline_protocol_send_message(OfflineProtocol *handle,
                                      const char *recipient,
                                      const char *content,
                                      int32_t priority,
                                      char *out_message_id,
                                      uintptr_t out_len);

/**
 * Polls for the next event.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_event_json` must point to a buffer of at least `out_len` bytes.
 *
 * # Returns
 *
 * Returns SUCCESS if an event was retrieved, 0 if no event available, or an error code.
 */
int32_t offline_protocol_poll_event(OfflineProtocol *handle,
                                    char *out_event_json,
                                    uintptr_t _out_len);

/**
 * Frees a string allocated by the FFI layer.
 *
 * # Safety
 *
 * `s` must be a pointer returned by an FFI function that allocates strings.
 */
void offline_protocol_free_string(char *s);

#endif /* OFFLINE_PROTOCOL_H */
