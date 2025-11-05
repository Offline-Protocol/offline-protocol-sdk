//! BLE Bridge - Connects Rust BLE transport with Android BLE implementation
//!
//! This native bridge allows the Rust core to control Android BLE operations.

#include <jni.h>
#include <string>
#include <android/log.h>

#define LOG_TAG "BLE_Bridge"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

extern "C" {

// Global reference to the BleManager instance
static jobject g_ble_manager = nullptr;
static JavaVM* g_jvm = nullptr;

/**
 * Initialize the BLE bridge
 * Called from Kotlin with the BleManager instance
 */
JNIEXPORT void JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeInitBleBridge(
    JNIEnv* env,
    jobject /* this */,
    jobject ble_manager
) {
    LOGI("Initializing BLE bridge");
    
    // Get JavaVM for later JNI calls
    if (g_jvm == nullptr) {
        env->GetJavaVM(&g_jvm);
    }
    
    // Store global reference to BleManager
    if (g_ble_manager != nullptr) {
        env->DeleteGlobalRef(g_ble_manager);
    }
    g_ble_manager = env->NewGlobalRef(ble_manager);
    
    LOGI("BLE bridge initialized");
}

/**
 * Start BLE operations (advertising + scanning)
 */
JNIEXPORT jboolean JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeStartBle(
    JNIEnv* env,
    jobject /* this */
) {
    if (g_ble_manager == nullptr) {
        LOGE("BLE manager not initialized");
        return JNI_FALSE;
    }
    
    // Call BleManager.start()
    jclass bleManagerClass = env->GetObjectClass(g_ble_manager);
    jmethodID startMethod = env->GetMethodID(bleManagerClass, "start", "()Z");
    
    if (startMethod == nullptr) {
        LOGE("Failed to find start method");
        return JNI_FALSE;
    }
    
    jboolean result = env->CallBooleanMethod(g_ble_manager, startMethod);
    LOGI("BLE start result: %d", result);
    
    return result;
}

/**
 * Stop BLE operations
 */
JNIEXPORT void JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeStopBle(
    JNIEnv* env,
    jobject /* this */
) {
    if (g_ble_manager == nullptr) {
        LOGE("BLE manager not initialized");
        return;
    }
    
    // Call BleManager.stop()
    jclass bleManagerClass = env->GetObjectClass(g_ble_manager);
    jmethodID stopMethod = env->GetMethodID(bleManagerClass, "stop", "()V");
    
    if (stopMethod == nullptr) {
        LOGE("Failed to find stop method");
        return;
    }
    
    env->CallVoidMethod(g_ble_manager, stopMethod);
    LOGI("BLE stopped");
}

/**
 * Send message to a peer
 */
JNIEXPORT jboolean JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeSendBleMessage(
    JNIEnv* env,
    jobject /* this */,
    jstring recipient_id,
    jbyteArray message_data
) {
    if (g_ble_manager == nullptr) {
        LOGE("BLE manager not initialized");
        return JNI_FALSE;
    }
    
    // Call BleManager.sendMessage()
    jclass bleManagerClass = env->GetObjectClass(g_ble_manager);
    jmethodID sendMethod = env->GetMethodID(
        bleManagerClass,
        "sendMessage",
        "(Ljava/lang/String;[B)Z"
    );
    
    if (sendMethod == nullptr) {
        LOGE("Failed to find sendMessage method");
        return JNI_FALSE;
    }
    
    jboolean result = env->CallBooleanMethod(
        g_ble_manager,
        sendMethod,
        recipient_id,
        message_data
    );
    
    return result;
}

/**
 * Cleanup
 */
JNIEXPORT void JNICALL
Java_com_offlineprotocol_OfflineProtocolModule_nativeCleanupBleBridge(
    JNIEnv* env,
    jobject /* this */
) {
    if (g_ble_manager != nullptr) {
        env->DeleteGlobalRef(g_ble_manager);
        g_ble_manager = nullptr;
    }
    LOGI("BLE bridge cleaned up");
}

} // extern "C"

