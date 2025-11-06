//
//  offline_protocol_bridging.h
//  OfflineProtocol
//
//  Bridging header for Rust FFI functions and React Native imports
//

#ifndef offline_protocol_bridging_h
#define offline_protocol_bridging_h

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

// Success code
#define SUCCESS 0
#define NO_FRAGMENT_AVAILABLE 1
#define NO_MESSAGE_AVAILABLE 2

// Error codes
#define ERROR_NULL_POINTER -1
#define ERROR_INVALID_UTF8 -2
#define ERROR_NOT_STARTED -3
#define ERROR_ALREADY_STARTED -4
#define ERROR_SEND_FAILED -5
#define ERROR_INVALID_CONFIG -6
#define ERROR_INVALID_STATE -7
#define ERROR_PANIC -99
#define ERROR_OTHER -100

// Opaque handle type
typedef struct ProtocolHandle ProtocolHandle;

// Event callback function type
typedef void (*EventCallback)(const char *event_json, void *user_data);

// Option type for event callback (matches Rust Option representation)
typedef struct Option_EventCallback {
  bool is_some;
  EventCallback value;
} Option_EventCallback;

// FFI functions
ProtocolHandle *offline_protocol_create(const char *config_json);
void offline_protocol_destroy(ProtocolHandle *handle);
int32_t offline_protocol_start(ProtocolHandle *handle);
int32_t offline_protocol_stop(ProtocolHandle *handle);
int32_t offline_protocol_send_message(ProtocolHandle *handle,
                                      const char *recipient,
                                      const char *content,
                                      int32_t priority,
                                      char *out_message_id,
                                      uintptr_t out_len);
int32_t offline_protocol_set_event_callback(ProtocolHandle *handle,
                                            Option_EventCallback callback,
                                            void *user_data);
void offline_protocol_free_string(char *s);

// BLE transport notification functions
int32_t offline_protocol_ble_peer_discovered(ProtocolHandle *handle,
                                              const char *device_id,
                                              const char *address,
                                              int16_t rssi);
int32_t offline_protocol_ble_peer_lost(ProtocolHandle *handle,
                                        const char *device_id);
int32_t offline_protocol_ble_status_changed(ProtocolHandle *handle,
                                             int32_t status);
int32_t offline_protocol_ble_get_peer_count(ProtocolHandle *handle);
int32_t offline_protocol_ble_fragment_received(ProtocolHandle *handle,
                                               const uint8_t *fragment_data,
                                               uintptr_t data_len);
int32_t offline_protocol_ble_get_next_fragment(ProtocolHandle *handle,
                                               char *recipient_out,
                                               uintptr_t recipient_out_len,
                                               uint8_t *fragment_out,
                                               uintptr_t fragment_out_len,
                                               uintptr_t *fragment_len_out);
int32_t offline_protocol_ble_return_fragment(ProtocolHandle *handle,
                                             const char *recipient,
                                             const uint8_t *fragment_data,
                                             uintptr_t fragment_len);

int32_t offline_protocol_update_transport_metrics(ProtocolHandle *handle,
                                                   int32_t transport_type,
                                                   int16_t rssi,
                                                   uint32_t latency_ms,
                                                   uint64_t bandwidth_bps,
                                                   float congestion,
                                                   uintptr_t queue_depth,
                                                   uint32_t success_count,
                                                   uint32_t failure_count);

int32_t offline_protocol_should_escalate_to_wifi(ProtocolHandle *handle,
                                                  int32_t *out_should_escalate);

int32_t offline_protocol_add_internet_transport(ProtocolHandle *handle,
                                                 const char *config_json);

int32_t offline_protocol_add_wifi_direct_transport(ProtocolHandle *handle,
                                                    const char *config_json);

// Visualization functions
int32_t offline_protocol_get_topology(ProtocolHandle *handle,
                                      char *out_buffer,
                                      uintptr_t buffer_len);
int32_t offline_protocol_get_message_stats(ProtocolHandle *handle,
                                           char *out_buffer,
                                           uintptr_t buffer_len);
int32_t offline_protocol_get_delivery_success_rate(ProtocolHandle *handle,
                                                   float *out_rate);
int32_t offline_protocol_get_median_latency(ProtocolHandle *handle,
                                            uint64_t *out_latency);
int32_t offline_protocol_get_median_hops(ProtocolHandle *handle,
                                         uint8_t *out_hops);

// File transfer functions
int32_t offline_protocol_send_file(ProtocolHandle *handle,
                                   const uint8_t *file_data,
                                   uintptr_t file_data_len,
                                   const char *file_name,
                                   const char *recipient,
                                   char *out_file_id,
                                   uintptr_t out_file_id_len);

int32_t offline_protocol_get_file_progress(ProtocolHandle *handle,
                                           const char *file_id,
                                           char *out_progress_json,
                                           uintptr_t out_len);

int32_t offline_protocol_cancel_file_transfer(ProtocolHandle *handle,
                                              const char *file_id);

// Process and state management
int32_t offline_protocol_process(ProtocolHandle *handle);
int32_t offline_protocol_pause(ProtocolHandle *handle);
int32_t offline_protocol_resume(ProtocolHandle *handle);
int32_t offline_protocol_get_state(ProtocolHandle *handle);

// Message polling
int32_t offline_protocol_receive_message(ProtocolHandle *handle,
                                         char *out_message_json,
                                         uintptr_t out_len);

// Transport management
int32_t offline_protocol_remove_transport(ProtocolHandle *handle,
                                          int32_t transport_type);
int32_t offline_protocol_get_active_transports(ProtocolHandle *handle,
                                               char *out_buffer,
                                               uintptr_t buffer_len);

#endif /* offline_protocol_bridging_h */

