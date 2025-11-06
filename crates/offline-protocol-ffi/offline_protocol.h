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
 * No BLE fragment available.
 */
#define NO_FRAGMENT_AVAILABLE 1

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

typedef struct Option_EventCallback Option_EventCallback;

/**
 * Wrapper for OfflineProtocol with event callback support.
 *
 * This is an opaque type used only via pointers in the FFI interface.
 */
typedef struct ProtocolHandle ProtocolHandle;

/**
 * Creates a new OfflineProtocol instance from JSON configuration.
 *
 * # Safety
 *
 * `config_json` must be a valid null-terminated C string.
 * Returns a pointer to ProtocolHandle on success, or null on failure.
 */
struct ProtocolHandle *offline_protocol_create(const char *config_json);

/**
 * Destroys an OfflineProtocol instance and frees its memory.
 *
 * # Safety
 *
 * `handle` must be a valid pointer returned by `offline_protocol_create`.
 * After calling this function, the handle is invalid and must not be used.
 */
void offline_protocol_destroy(struct ProtocolHandle *handle);

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
int32_t offline_protocol_start(struct ProtocolHandle *handle);

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
int32_t offline_protocol_stop(struct ProtocolHandle *handle);

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
int32_t offline_protocol_send_message(struct ProtocolHandle *handle,
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
int32_t offline_protocol_poll_event(struct ProtocolHandle *handle,
                                    char *out_event_json,
                                    uintptr_t _out_len);

/**
 * Sets an event callback to receive protocol events.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `callback` must be a valid C function pointer with the EventCallback signature.
 * - `user_data` is an opaque pointer that will be passed back to the callback.
 * - The callback must be thread-safe as it may be invoked from any thread.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_set_event_callback(struct ProtocolHandle *handle,
                                            struct Option_EventCallback callback,
                                            void *user_data);

/**
 * Frees a string allocated by the FFI layer.
 *
 * # Safety
 *
 * `s` must be a pointer returned by an FFI function that allocates strings.
 */
void offline_protocol_free_string(char *s);

/**
 * Notifies the BLE transport that a peer has been discovered.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `device_id` and `address` must be valid null-terminated C strings.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_ble_peer_discovered(struct ProtocolHandle *handle,
                                             const char *device_id,
                                             const char *address,
                                             int16_t rssi);

/**
 * Notifies the BLE transport that a peer has been lost.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `device_id` must be a valid null-terminated C string.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_ble_peer_lost(struct ProtocolHandle *handle, const char *device_id);

/**
 * Notifies the BLE transport of a status change.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 *
 * # Arguments
 *
 * - `status`: 0 = Unavailable, 1 = Available, 2 = Disconnected
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_ble_status_changed(struct ProtocolHandle *handle, int32_t status);

/**
 * Called when a BLE fragment is received from a peer.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `fragment_data` must be a valid pointer to a byte array of length `data_len`.
 *
 * # Arguments
 *
 * - `fragment_data`: Pointer to the fragment byte array
 * - `data_len`: Length of the fragment data
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_ble_fragment_received(struct ProtocolHandle *handle,
                                               const uint8_t *fragment_data,
                                               uintptr_t data_len);

/**
 * Gets the next BLE fragment to send.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `recipient_out` must be a valid pointer to a buffer of at least 256 bytes.
 * - `fragment_out` must be a valid pointer to a buffer of at least `fragment_out_len` bytes.
 * - `fragment_len_out` must be a valid pointer to store the actual fragment length.
 *
 * # Arguments
 *
 * - `recipient_out`: Buffer to write the recipient device ID (null-terminated)
 * - `recipient_out_len`: Size of the recipient buffer
 * - `fragment_out`: Buffer to write the fragment data
 * - `fragment_out_len`: Size of the fragment buffer
 * - `fragment_len_out`: Pointer to store the actual fragment length
 *
 * # Returns
 *
 * Returns SUCCESS if a fragment is available, NO_FRAGMENT_AVAILABLE if none are queued, or an error code on failure.
 */
int32_t offline_protocol_ble_get_next_fragment(struct ProtocolHandle *handle,
                                               char *recipient_out,
                                               uintptr_t recipient_out_len,
                                               uint8_t *fragment_out,
                                               uintptr_t fragment_out_len,
                                               uintptr_t *fragment_len_out);

/**
 * Re-queues a BLE fragment if sending fails on the platform side.
 */
int32_t offline_protocol_ble_return_fragment(struct ProtocolHandle *handle,
                                             const char *recipient,
                                             const uint8_t *fragment_data,
                                             uintptr_t fragment_len);

/**
 * Gets the number of discovered peers.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 *
 * # Returns
 *
 * Returns the number of discovered peers, or -1 on error.
 */
int32_t offline_protocol_ble_get_peer_count(struct ProtocolHandle *handle);

/**
 * Adds an Internet transport to the protocol.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `config_json` must be a valid null-terminated C string containing JSON configuration.
 *
 * Configuration JSON format:
 * ```json
 * {
 *   "serverAddress": "wss://relay.example.com",
 *   "autoReconnect": true,
 *   "reconnectDelay": 5000
 * }
 * ```
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_add_internet_transport(struct ProtocolHandle *handle,
                                                const char *config_json);

/**
 * Adds a WiFi Direct transport to the protocol.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `config_json` must be a valid null-terminated C string containing JSON configuration.
 *
 * Configuration JSON format:
 * ```json
 * {
 *   "deviceName": "MyDevice",
 *   "autoAccept": false,
 *   "groupOwnerIntent": 7
 * }
 * ```
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_add_wifi_direct_transport(struct ProtocolHandle *handle,
                                                   const char *config_json);

/**
 * Removes a transport from the protocol by type.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `transport_type` must be one of: 0 (Internet), 1 (BLE), 2 (WiFiDirect).
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_remove_transport(struct ProtocolHandle *handle, int32_t transport_type);

/**
 * Gets the list of active transports.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_buffer` must be a valid pointer to a buffer of at least `buffer_len` bytes.
 *
 * The output format is a JSON array of transport names, e.g.:
 * `["ble", "internet"]`
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_get_active_transports(struct ProtocolHandle *handle,
                                               char *out_buffer,
                                               uintptr_t buffer_len);

/**
 * Gets the current network topology as JSON.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_buffer` must be a valid pointer to a buffer of at least `buffer_len` bytes.
 *
 * The output is a JSON string containing the complete network topology including nodes, links, and stats.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_get_topology(struct ProtocolHandle *handle,
                                      char *out_buffer,
                                      uintptr_t buffer_len);

/**
 * Gets message delivery statistics as JSON.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_buffer` must be a valid pointer to a buffer of at least `buffer_len` bytes.
 *
 * The output is a JSON array containing message statistics.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_get_message_stats(struct ProtocolHandle *handle,
                                           char *out_buffer,
                                           uintptr_t buffer_len);

/**
 * Gets delivery success rate (0.0 - 1.0).
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_rate` must be a valid pointer to store the success rate.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_get_delivery_success_rate(struct ProtocolHandle *handle, float *out_rate);

/**
 * Gets median delivery latency in milliseconds.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_latency` must be a valid pointer to store the latency.
 *
 * # Returns
 *
 * Returns SUCCESS if latency is available, 0 if no data, or an error code on failure.
 */
int32_t offline_protocol_get_median_latency(struct ProtocolHandle *handle, uint64_t *out_latency);

/**
 * Gets median hop count.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_hops` must be a valid pointer to store the hop count.
 *
 * # Returns
 *
 * Returns SUCCESS if hop count is available, 0 if no data, or an error code on failure.
 */
int32_t offline_protocol_get_median_hops(struct ProtocolHandle *handle, uint8_t *out_hops);

/**
 * Updates transport metrics for DORS scoring.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `transport_type` must be one of: 0 (Internet), 1 (BLE), 2 (WiFiDirect).
 *
 * # Arguments
 *
 * - `rssi`: Signal strength in dBm (or -1 if not applicable)
 * - `latency_ms`: Latency in milliseconds (or 0 if not applicable)
 * - `bandwidth_bps`: Bandwidth in bytes per second (or 0 if not applicable)
 * - `congestion`: Congestion level from 0.0 to 1.0
 * - `queue_depth`: Number of messages in send queue
 * - `success_count`: Number of successful sends in last window
 * - `failure_count`: Number of failed sends in last window
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 */
int32_t offline_protocol_update_transport_metrics(struct ProtocolHandle *handle,
                                                  int32_t transport_type,
                                                  int16_t rssi,
                                                  uint32_t latency_ms,
                                                  uint64_t bandwidth_bps,
                                                  float congestion,
                                                  uintptr_t queue_depth,
                                                  uint32_t success_count,
                                                  uint32_t failure_count);

/**
 * Checks if DORS should escalate from BLE to Wi-Fi Direct.
 *
 * # Safety
 *
 * - `handle` must be a valid pointer returned by `offline_protocol_create`.
 * - `out_should_escalate` must be a valid pointer to store the result.
 *
 * # Returns
 *
 * Returns SUCCESS on success, or an error code on failure.
 * Sets `out_should_escalate` to 1 if escalation is needed, 0 otherwise.
 */
int32_t offline_protocol_should_escalate_to_wifi(struct ProtocolHandle *handle,
                                                 int32_t *out_should_escalate);

#endif /* OFFLINE_PROTOCOL_H */
