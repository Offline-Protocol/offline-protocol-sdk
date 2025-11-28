//
// offline_protocol_jni.cpp
// OfflineProtocol JNI wrapper
//

#include <jni.h>
#include <android/log.h>
#include <string>
#include <cstring>
#include <stdint.h>

#define LOG_TAG "OfflineProtocolJNI"
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

// Import FFI functions
extern "C" {
    typedef struct ProtocolHandle ProtocolHandle;
    typedef void (*EventCallback)(const char *event_json, void *user_data);
    typedef struct Option_EventCallback {
        bool is_some;
        EventCallback value;
    } Option_EventCallback;
    
    ProtocolHandle* offline_protocol_create(const char* config_json);
    void offline_protocol_destroy(ProtocolHandle* handle);
    int32_t offline_protocol_start(ProtocolHandle* handle);
    int32_t offline_protocol_stop(ProtocolHandle* handle);
    int32_t offline_protocol_send_message(ProtocolHandle* handle,
                                          const char* recipient,
                                          const char* content,
                                          int32_t priority,
                                          char* out_message_id,
                                          uintptr_t out_len);
    int32_t offline_protocol_set_event_callback(ProtocolHandle* handle,
                                                Option_EventCallback callback,
                                                void* user_data);
    void offline_protocol_free_string(char* s);
    
    // BLE transport notification functions
    int32_t offline_protocol_ble_peer_discovered(ProtocolHandle* handle,
                                                  const char* device_id,
                                                  const char* address,
                                                  int16_t rssi);
    int32_t offline_protocol_ble_peer_lost(ProtocolHandle* handle,
                                            const char* device_id);
    int32_t offline_protocol_ble_status_changed(ProtocolHandle* handle,
                                                 int32_t status);
    int32_t offline_protocol_ble_get_peer_count(ProtocolHandle* handle);
    int32_t offline_protocol_ble_fragment_received(ProtocolHandle* handle,
                                                   const uint8_t* fragment_data,
                                                   uintptr_t data_len);
    int32_t offline_protocol_ble_get_next_fragment(ProtocolHandle* handle,
                                                   char* recipient_out,
                                                   uintptr_t recipient_out_len,
                                                   uint8_t* fragment_out,
                                                   uintptr_t fragment_out_len,
                                                   uintptr_t* fragment_len_out);
    int32_t offline_protocol_ble_return_fragment(ProtocolHandle* handle,
                                                 const char* recipient,
                                                 const uint8_t* fragment_data,
                                                 uintptr_t fragment_len);
    
    // Visualization functions
    int32_t offline_protocol_get_topology(ProtocolHandle* handle,
                                          char* out_buffer,
                                          uintptr_t buffer_len);
    int32_t offline_protocol_get_message_stats(ProtocolHandle* handle,
                                               char* out_buffer,
                                               uintptr_t buffer_len);
    int32_t offline_protocol_get_delivery_success_rate(ProtocolHandle* handle,
                                                       float* out_rate);
    int32_t offline_protocol_get_median_latency(ProtocolHandle* handle,
                                                uint64_t* out_latency);
    int32_t offline_protocol_get_median_hops(ProtocolHandle* handle,
                                             uint8_t* out_hops);
    int32_t offline_protocol_update_transport_metrics(ProtocolHandle* handle,
                                                       int32_t transport_type,
                                                       int16_t rssi,
                                                       uint32_t latency_ms,
                                                       uint64_t bandwidth_bps,
                                                       float congestion,
                                                       uintptr_t queue_depth,
                                                       uint32_t success_count,
                                                       uint32_t failure_count);
    int32_t offline_protocol_should_escalate_to_wifi(ProtocolHandle* handle,
                                                      int32_t* out_should_escalate);
    int32_t offline_protocol_add_internet_transport(ProtocolHandle* handle,
                                                     const char* config_json);
    int32_t offline_protocol_add_wifi_direct_transport(ProtocolHandle* handle,
                                                        const char* config_json);
    int32_t offline_protocol_remove_transport(ProtocolHandle* handle,
                                              int32_t transport_type);
    int32_t offline_protocol_get_active_transports(ProtocolHandle* handle,
                                                   char* out_buffer,
                                                   uintptr_t buffer_len);
    
    // File transfer functions
    int32_t offline_protocol_send_file(ProtocolHandle* handle,
                                       const uint8_t* file_data,
                                       uintptr_t file_data_len,
                                       const char* file_name,
                                       const char* recipient,
                                       char* out_file_id,
                                       uintptr_t out_file_id_len);
    int32_t offline_protocol_get_file_progress(ProtocolHandle* handle,
                                               const char* file_id,
                                               char* out_progress_json,
                                               uintptr_t out_len);
    int32_t offline_protocol_cancel_file_transfer(ProtocolHandle* handle,
                                                  const char* file_id);
    
    // Process and state management
    int32_t offline_protocol_process(ProtocolHandle* handle);
    int32_t offline_protocol_pause(ProtocolHandle* handle);
    int32_t offline_protocol_resume(ProtocolHandle* handle);
    int32_t offline_protocol_get_state(ProtocolHandle* handle);
    
    // Message polling
    int32_t offline_protocol_receive_message(ProtocolHandle* handle,
                                            char* out_message_json,
                                            uintptr_t out_len);
}

// Error codes
const int32_t SUCCESS = 0;
const int32_t ERROR_NULL_POINTER = -1;
const int32_t ERROR_NOT_STARTED = -3;
const int32_t ERROR_ALREADY_STARTED = -4;
const int32_t ERROR_SEND_FAILED = -5;
const int32_t ERROR_OTHER = -100;
const int32_t NO_FRAGMENT_AVAILABLE = 1;
const int32_t NO_MESSAGE_AVAILABLE = 2;

// Global callback context
struct CallbackContext {
    JavaVM* jvm;
    jobject moduleRef;
};

// Event callback handler
void eventCallbackHandler(const char* event_json, void* user_data) {
    if (!event_json || !user_data) {
        return;
    }
    
    CallbackContext* context = static_cast<CallbackContext*>(user_data);
    JNIEnv* env = nullptr;
    
    // Attach current thread to JVM
    bool needsDetach = false;
    int getEnvStat = context->jvm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
    if (getEnvStat == JNI_EDETACHED) {
        if (context->jvm->AttachCurrentThread(&env, nullptr) != 0) {
            LOGE("Failed to attach thread to JVM");
            return;
        }
        needsDetach = true;
    }
    
    // Call Java method
    jclass moduleClass = env->GetObjectClass(context->moduleRef);
    jmethodID method = env->GetMethodID(moduleClass, "handleEvent", "(Ljava/lang/String;)V");
    
    if (method) {
        jstring eventStr = env->NewStringUTF(event_json);
        env->CallVoidMethod(context->moduleRef, method, eventStr);
        env->DeleteLocalRef(eventStr);
    }
    
    env->DeleteLocalRef(moduleClass);
    
    // Detach thread if we attached it
    if (needsDetach) {
        context->jvm->DetachCurrentThread();
    }
}

// JNI Methods
extern "C" {

JNIEXPORT jlong JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeCreate(
    JNIEnv* env, jobject thiz, jstring configJson) {
    
    const char* config = env->GetStringUTFChars(configJson, nullptr);
    ProtocolHandle* handle = offline_protocol_create(config);
    env->ReleaseStringUTFChars(configJson, config);
    
    if (!handle) {
        return 0;
    }
    
    // Set up callback context
    CallbackContext* context = new CallbackContext();
    env->GetJavaVM(&context->jvm);
    context->moduleRef = env->NewGlobalRef(thiz);
    
    // Set event callback
    Option_EventCallback callbackOption;
    callbackOption.is_some = true;
    callbackOption.value = eventCallbackHandler;
    int32_t result = offline_protocol_set_event_callback(
        handle, callbackOption, context);
    
    if (result != SUCCESS) {
        env->DeleteGlobalRef(context->moduleRef);
        delete context;
        offline_protocol_destroy(handle);
        return 0;
    }
    
    // Store callback context in handle (we'll manage lifetime)
    // For now, we'll leak the context - proper cleanup would need a wrapper struct
    
    return reinterpret_cast<jlong>(handle);
}

JNIEXPORT void JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeDestroy(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    offline_protocol_destroy(handle);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeStart(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_start(handle);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeStop(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_stop(handle);
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeSendMessage(
    JNIEnv* env, jobject thiz, jlong handlePtr,
    jstring recipient, jstring content, jint priority) {
    
    if (handlePtr == 0) {
        LOGE("Invalid handle pointer");
        return nullptr;
    }
    
    if (recipient == nullptr || content == nullptr) {
        LOGE("Null recipient or content");
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* recipientStr = env->GetStringUTFChars(recipient, nullptr);
    if (recipientStr == nullptr) {
        LOGE("Failed to get recipient string");
        return nullptr;
    }
    
    const char* contentStr = env->GetStringUTFChars(content, nullptr);
    if (contentStr == nullptr) {
        LOGE("Failed to get content string");
        env->ReleaseStringUTFChars(recipient, recipientStr);
        return nullptr;
    }
    
    char messageId[256];
    memset(messageId, 0, sizeof(messageId)); // Initialize buffer
    
    int32_t result = offline_protocol_send_message(
        handle, recipientStr, contentStr, priority, messageId, 256);
    
    env->ReleaseStringUTFChars(recipient, recipientStr);
    env->ReleaseStringUTFChars(content, contentStr);
    
    if (result != SUCCESS) {
        LOGE("send_message failed with code: %d", result);
        return nullptr;
    }
    
    jstring resultStr = env->NewStringUTF(messageId);
    if (resultStr == nullptr) {
        LOGE("Failed to create result string");
    }
    
    return resultStr;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBlePeerDiscovered(
    JNIEnv* env, jobject thiz, jlong handlePtr,
    jstring deviceId, jstring address, jshort rssi) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* deviceIdStr = env->GetStringUTFChars(deviceId, nullptr);
    const char* addressStr = env->GetStringUTFChars(address, nullptr);
    
    int32_t result = offline_protocol_ble_peer_discovered(
        handle, deviceIdStr, addressStr, static_cast<int16_t>(rssi));
    
    env->ReleaseStringUTFChars(deviceId, deviceIdStr);
    env->ReleaseStringUTFChars(address, addressStr);
    
    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBlePeerLost(
    JNIEnv* env, jobject thiz, jlong handlePtr, jstring deviceId) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* deviceIdStr = env->GetStringUTFChars(deviceId, nullptr);
    int32_t result = offline_protocol_ble_peer_lost(handle, deviceIdStr);
    env->ReleaseStringUTFChars(deviceId, deviceIdStr);
    
    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBleStatusChanged(
    JNIEnv* env, jobject thiz, jlong handlePtr, jint status) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_ble_status_changed(handle, static_cast<int32_t>(status));
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBleGetPeerCount(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return -1;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_ble_get_peer_count(handle);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBleFragmentReceived(
    JNIEnv* env, jobject /* thiz */, jlong handlePtr, jbyteArray fragmentData) {

    if (handlePtr == 0 || fragmentData == nullptr) {
        return ERROR_NULL_POINTER;
    }

    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    jsize length = env->GetArrayLength(fragmentData);

    jbyte* data = env->GetByteArrayElements(fragmentData, nullptr);
    int32_t result = offline_protocol_ble_fragment_received(
        handle,
        reinterpret_cast<uint8_t*>(data),
        static_cast<uintptr_t>(length)
    );
    env->ReleaseByteArrayElements(fragmentData, data, JNI_ABORT);

    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBleGetNextFragment(
    JNIEnv* env, jobject /* thiz */, jlong handlePtr, jbyteArray recipientBuffer, jbyteArray fragmentBuffer) {

    if (handlePtr == 0 || recipientBuffer == nullptr || fragmentBuffer == nullptr) {
        return ERROR_NULL_POINTER;
    }

    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);

    jsize recipientLen = env->GetArrayLength(recipientBuffer);
    jsize fragmentLen = env->GetArrayLength(fragmentBuffer);

    jbyte* recipientData = env->GetByteArrayElements(recipientBuffer, nullptr);
    jbyte* fragmentData = env->GetByteArrayElements(fragmentBuffer, nullptr);

    uintptr_t outLen = 0;
    int32_t result = offline_protocol_ble_get_next_fragment(
        handle,
        reinterpret_cast<char*>(recipientData),
        static_cast<uintptr_t>(recipientLen),
        reinterpret_cast<uint8_t*>(fragmentData),
        static_cast<uintptr_t>(fragmentLen),
        &outLen
    );

    env->ReleaseByteArrayElements(recipientBuffer, recipientData, 0);
    env->ReleaseByteArrayElements(fragmentBuffer, fragmentData, 0);

    if (result == SUCCESS) {
        if (outLen > static_cast<uintptr_t>(fragmentLen)) {
            return ERROR_OTHER;
        }
        return static_cast<jint>(outLen);
    }

    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeBleReturnFragment(
    JNIEnv* env, jobject /* thiz */, jlong handlePtr, jstring recipient, jbyteArray fragmentData, jint fragmentLength) {

    if (handlePtr == 0 || recipient == nullptr || fragmentData == nullptr) {
        return ERROR_NULL_POINTER;
    }

    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);

    const char* recipientStr = env->GetStringUTFChars(recipient, nullptr);
    jbyte* data = env->GetByteArrayElements(fragmentData, nullptr);

    int32_t result = offline_protocol_ble_return_fragment(
        handle,
        recipientStr,
        reinterpret_cast<uint8_t*>(data),
        static_cast<uintptr_t>(fragmentLength)
    );

    env->ReleaseStringUTFChars(recipient, recipientStr);
    env->ReleaseByteArrayElements(fragmentData, data, JNI_ABORT);

    return result;
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetTopology(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    char buffer[65536];
    int32_t result = offline_protocol_get_topology(handle, buffer, sizeof(buffer));
    
    if (result != SUCCESS) {
        return nullptr;
    }
    
    return env->NewStringUTF(buffer);
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetMessageStats(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    char buffer[65536];
    int32_t result = offline_protocol_get_message_stats(handle, buffer, sizeof(buffer));
    
    if (result != SUCCESS) {
        return nullptr;
    }
    
    return env->NewStringUTF(buffer);
}

JNIEXPORT jfloat JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetDeliverySuccessRate(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return 0.0f;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    float rate = 0.0f;
    int32_t result = offline_protocol_get_delivery_success_rate(handle, &rate);
    
    if (result != SUCCESS) {
        return 0.0f;
    }
    
    return rate;
}

JNIEXPORT jlong JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetMedianLatency(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return -1;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    uint64_t latency = 0;
    int32_t result = offline_protocol_get_median_latency(handle, &latency);
    
    if (result == 0) {
        return -1; // No data available
    } else if (result != SUCCESS) {
        return -1; // Error
    }
    
    return static_cast<jlong>(latency);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetMedianHops(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return -1;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    uint8_t hops = 0;
    int32_t result = offline_protocol_get_median_hops(handle, &hops);
    
    if (result == 0) {
        return -1; // No data available
    } else if (result != SUCCESS) {
        return -1; // Error
    }
    
    return static_cast<jint>(hops);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeUpdateTransportMetrics(
    JNIEnv* env, jobject thiz, jlong handlePtr,
    jint transportType, jshort rssi, jint latencyMs,
    jlong bandwidthBps, jfloat congestion, jint queueDepth,
    jint successCount, jint failureCount) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    return offline_protocol_update_transport_metrics(
        handle,
        static_cast<int32_t>(transportType),
        static_cast<int16_t>(rssi),
        static_cast<uint32_t>(latencyMs),
        static_cast<uint64_t>(bandwidthBps),
        static_cast<float>(congestion),
        static_cast<uintptr_t>(queueDepth),
        static_cast<uint32_t>(successCount),
        static_cast<uint32_t>(failureCount)
    );
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeShouldEscalateToWifi(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return -1;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    int32_t shouldEscalate = 0;
    int32_t result = offline_protocol_should_escalate_to_wifi(handle, &shouldEscalate);
    
    if (result != SUCCESS) {
        return -1;
    }
    
    return static_cast<jint>(shouldEscalate);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeAddInternetTransport(
    JNIEnv* env, jobject thiz, jlong handlePtr, jstring configJson) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* config = nullptr;
    if (configJson != nullptr) {
        config = env->GetStringUTFChars(configJson, nullptr);
    }
    
    int32_t result = offline_protocol_add_internet_transport(handle, config);
    
    if (config != nullptr) {
        env->ReleaseStringUTFChars(configJson, config);
    }
    
    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeAddWifiDirectTransport(
    JNIEnv* env, jobject thiz, jlong handlePtr, jstring configJson) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* config = nullptr;
    if (configJson != nullptr) {
        config = env->GetStringUTFChars(configJson, nullptr);
    }
    
    int32_t result = offline_protocol_add_wifi_direct_transport(handle, config);
    
    if (config != nullptr) {
        env->ReleaseStringUTFChars(configJson, config);
    }
    
    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeRemoveTransport(
    JNIEnv* env, jobject thiz, jlong handlePtr, jint transportType) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_remove_transport(handle, static_cast<int32_t>(transportType));
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetActiveTransports(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    char buffer[4096];
    int32_t result = offline_protocol_get_active_transports(handle, buffer, sizeof(buffer));
    
    if (result != SUCCESS) {
        return nullptr;
    }
    
    return env->NewStringUTF(buffer);
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeSendFile(
    JNIEnv* env, jobject thiz, jlong handlePtr, jbyteArray fileData,
    jstring fileName, jstring recipient) {
    
    if (handlePtr == 0 || fileData == nullptr || fileName == nullptr || recipient == nullptr) {
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    jsize fileDataLen = env->GetArrayLength(fileData);
    jbyte* fileBytes = env->GetByteArrayElements(fileData, nullptr);
    
    const char* fileNameStr = env->GetStringUTFChars(fileName, nullptr);
    const char* recipientStr = env->GetStringUTFChars(recipient, nullptr);
    
    char fileId[256];
    int32_t result = offline_protocol_send_file(
        handle,
        reinterpret_cast<uint8_t*>(fileBytes),
        static_cast<uintptr_t>(fileDataLen),
        fileNameStr,
        recipientStr,
        fileId,
        256
    );
    
    env->ReleaseByteArrayElements(fileData, fileBytes, JNI_ABORT);
    env->ReleaseStringUTFChars(fileName, fileNameStr);
    env->ReleaseStringUTFChars(recipient, recipientStr);
    
    if (result != SUCCESS) {
        return nullptr;
    }
    
    return env->NewStringUTF(fileId);
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetFileProgress(
    JNIEnv* env, jobject thiz, jlong handlePtr, jstring fileId) {
    
    if (handlePtr == 0 || fileId == nullptr) {
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* fileIdStr = env->GetStringUTFChars(fileId, nullptr);
    
    char buffer[4096];
    int32_t result = offline_protocol_get_file_progress(handle, fileIdStr, buffer, sizeof(buffer));
    
    env->ReleaseStringUTFChars(fileId, fileIdStr);
    
    if (result == 0) {
        return nullptr; // Not found
    } else if (result != SUCCESS) {
        return nullptr; // Error
    }
    
    return env->NewStringUTF(buffer);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeCancelFileTransfer(
    JNIEnv* env, jobject thiz, jlong handlePtr, jstring fileId) {
    
    if (handlePtr == 0 || fileId == nullptr) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* fileIdStr = env->GetStringUTFChars(fileId, nullptr);
    int32_t result = offline_protocol_cancel_file_transfer(handle, fileIdStr);
    env->ReleaseStringUTFChars(fileId, fileIdStr);
    
    return result;
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeProcess(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_process(handle);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativePause(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_pause(handle);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeResume(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_resume(handle);
}

JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeGetState(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return ERROR_NULL_POINTER;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    return offline_protocol_get_state(handle);
}

JNIEXPORT jstring JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeReceiveMessage(
    JNIEnv* env, jobject thiz, jlong handlePtr) {
    
    if (handlePtr == 0) {
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    char buffer[65536];
    int32_t result = offline_protocol_receive_message(handle, buffer, sizeof(buffer));
    
    if (result == NO_MESSAGE_AVAILABLE) {
        return nullptr; // No message available
    } else if (result != SUCCESS) {
        return nullptr; // Error
    }
    
    return env->NewStringUTF(buffer);
}

} // extern "C"

