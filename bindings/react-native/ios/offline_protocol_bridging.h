//
//  offline_protocol_bridging.h
//  OfflineProtocol
//
//  Bridging header for Rust FFI functions and React Native imports
//

#ifndef offline_protocol_bridging_h
#define offline_protocol_bridging_h

#include <stdint.h>
#include <stdlib.h>

// Success code
#define SUCCESS 0

// Error codes
#define ERROR_NULL_POINTER -1
#define ERROR_INVALID_UTF8 -2
#define ERROR_NOT_STARTED -3
#define ERROR_ALREADY_STARTED -4
#define ERROR_SEND_FAILED -5
#define ERROR_INVALID_CONFIG -6
#define ERROR_PANIC -99
#define ERROR_OTHER -100

// Opaque handle type
typedef struct ProtocolHandle ProtocolHandle;

// Event callback function type
typedef void (*EventCallback)(const char *event_json, void *user_data);

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
                                            EventCallback callback,
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

#endif /* offline_protocol_bridging_h */

