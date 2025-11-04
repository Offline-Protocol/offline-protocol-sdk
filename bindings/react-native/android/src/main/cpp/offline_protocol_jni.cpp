//
//  offline_protocol_jni.cpp
//  OfflineProtocol JNI wrapper
//

#include <jni.h>
#include <android/log.h>
#include <string>
#include <cstring>

#define LOG_TAG "OfflineProtocolJNI"
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

// Import FFI functions
extern "C" {
    typedef struct ProtocolHandle ProtocolHandle;
    typedef void (*EventCallback)(const char *event_json, void *user_data);
    
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
                                                EventCallback callback,
                                                void* user_data);
    void offline_protocol_free_string(char* s);
}

// Error codes
const int32_t SUCCESS = 0;
const int32_t ERROR_NULL_POINTER = -1;
const int32_t ERROR_NOT_STARTED = -3;
const int32_t ERROR_ALREADY_STARTED = -4;
const int32_t ERROR_SEND_FAILED = -5;

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
    int32_t result = offline_protocol_set_event_callback(
        handle, eventCallbackHandler, context);
    
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
        return nullptr;
    }
    
    ProtocolHandle* handle = reinterpret_cast<ProtocolHandle*>(handlePtr);
    
    const char* recipientStr = env->GetStringUTFChars(recipient, nullptr);
    const char* contentStr = env->GetStringUTFChars(content, nullptr);
    
    char messageId[256];
    int32_t result = offline_protocol_send_message(
        handle, recipientStr, contentStr, priority, messageId, 256);
    
    env->ReleaseStringUTFChars(recipient, recipientStr);
    env->ReleaseStringUTFChars(content, contentStr);
    
    if (result != SUCCESS) {
        return nullptr;
    }
    
    return env->NewStringUTF(messageId);
}

} // extern "C"

