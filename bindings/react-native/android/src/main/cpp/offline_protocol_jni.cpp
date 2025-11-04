#include <jni.h>
#include <string>
#include <cstring>
#include "offline_protocol.h"

// Global reference to JNI environment (we'll get it per call)
static JavaVM* g_jvm = nullptr;

// JNI function to get the current JNI environment
JNIEnv* getJNIEnv() {
    if (g_jvm == nullptr) {
        return nullptr;
    }
    JNIEnv* env = nullptr;
    jint result = g_jvm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
    if (result != JNI_OK || env == nullptr) {
        return nullptr;
    }
    return env;
}

extern "C" {

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
    g_jvm = vm;
    return JNI_VERSION_1_6;
}

JNIEXPORT void JNICALL JNI_OnUnload(JavaVM* vm, void* reserved) {
    g_jvm = nullptr;
}

/**
 * Creates a new OfflineProtocol instance.
 */
JNIEXPORT jlong JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeCreate(JNIEnv* env, jobject thiz, jstring configJson) {
    if (configJson == nullptr) {
        return 0;
    }

    const char* configStr = env->GetStringUTFChars(configJson, nullptr);
    if (configStr == nullptr) {
        return 0;
    }

    OfflineProtocol* protocol = offline_protocol_create(configStr);
    env->ReleaseStringUTFChars(configJson, configStr);

    return reinterpret_cast<jlong>(protocol);
}

/**
 * Destroys an OfflineProtocol instance.
 */
JNIEXPORT void JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeDestroy(JNIEnv* env, jobject thiz, jlong handle) {
    if (handle == 0) {
        return;
    }

    OfflineProtocol* protocol = reinterpret_cast<OfflineProtocol*>(handle);
    offline_protocol_destroy(protocol);
}

/**
 * Starts the protocol.
 */
JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeStart(JNIEnv* env, jobject thiz, jlong handle) {
    if (handle == 0) {
        return ERROR_NULL_POINTER;
    }

    OfflineProtocol* protocol = reinterpret_cast<OfflineProtocol*>(handle);
    return offline_protocol_start(protocol);
}

/**
 * Stops the protocol.
 */
JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeStop(JNIEnv* env, jobject thiz, jlong handle) {
    if (handle == 0) {
        return ERROR_NULL_POINTER;
    }

    OfflineProtocol* protocol = reinterpret_cast<OfflineProtocol*>(handle);
    return offline_protocol_stop(protocol);
}

/**
 * Sends a message.
 */
JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeSendMessage(
    JNIEnv* env,
    jobject thiz,
    jlong handle,
    jstring recipient,
    jstring content,
    jint priority,
    jbyteArray outMessageId,
    jint outLen
) {
    if (handle == 0 || recipient == nullptr || content == nullptr || outMessageId == nullptr) {
        return ERROR_NULL_POINTER;
    }

    const char* recipientStr = env->GetStringUTFChars(recipient, nullptr);
    const char* contentStr = env->GetStringUTFChars(content, nullptr);
    
    if (recipientStr == nullptr || contentStr == nullptr) {
        if (recipientStr != nullptr) {
            env->ReleaseStringUTFChars(recipient, recipientStr);
        }
        if (contentStr != nullptr) {
            env->ReleaseStringUTFChars(content, contentStr);
        }
        return ERROR_INVALID_UTF8;
    }

    // Allocate buffer for message ID
    char* messageIdBuffer = new char[outLen];
    memset(messageIdBuffer, 0, outLen);

    OfflineProtocol* protocol = reinterpret_cast<OfflineProtocol*>(handle);
    jint result = offline_protocol_send_message(
        protocol,
        recipientStr,
        contentStr,
        priority,
        messageIdBuffer,
        static_cast<uintptr_t>(outLen)
    );

    if (result == SUCCESS) {
        // Copy message ID to Java byte array
        env->SetByteArrayRegion(outMessageId, 0, static_cast<jsize>(strlen(messageIdBuffer)),
                                reinterpret_cast<const jbyte*>(messageIdBuffer));
    }

    delete[] messageIdBuffer;
    env->ReleaseStringUTFChars(recipient, recipientStr);
    env->ReleaseStringUTFChars(content, contentStr);

    return result;
}

/**
 * Polls for the next event.
 */
JNIEXPORT jint JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativePollEvent(
    JNIEnv* env,
    jobject thiz,
    jlong handle,
    jbyteArray outEventJson,
    jint outLen
) {
    if (handle == 0 || outEventJson == nullptr) {
        return ERROR_NULL_POINTER;
    }

    // Allocate buffer for event JSON
    char* eventBuffer = new char[outLen];
    memset(eventBuffer, 0, outLen);

    OfflineProtocol* protocol = reinterpret_cast<OfflineProtocol*>(handle);
    jint result = offline_protocol_poll_event(
        protocol,
        eventBuffer,
        static_cast<uintptr_t>(outLen)
    );

    if (result == SUCCESS) {
        // Copy event JSON to Java byte array
        jsize len = static_cast<jsize>(strlen(eventBuffer));
        env->SetByteArrayRegion(outEventJson, 0, len,
                                reinterpret_cast<const jbyte*>(eventBuffer));
    }

    delete[] eventBuffer;
    return result;
}

} // extern "C"
