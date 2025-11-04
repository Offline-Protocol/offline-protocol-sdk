cmake_minimum_required(VERSION 3.18)

project(offline_protocol_jni)

# Find React Native
set(REACT_NATIVE_DIR ${CMAKE_SOURCE_DIR}/../../../../../node_modules/react-native)
set(BUILD_DIR ${CMAKE_SOURCE_DIR}/build)

# Add JNI wrapper
add_library(offline_protocol_jni SHARED
    offline_protocol_jni.cpp
)

# Add pre-built Rust library
add_library(offline_protocol_ffi SHARED IMPORTED)
set_target_properties(offline_protocol_ffi PROPERTIES
    IMPORTED_LOCATION ${CMAKE_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/liboffline_protocol_ffi.so
)

# Link libraries
target_link_libraries(offline_protocol_jni
    offline_protocol_ffi
    log
    android
)

# Include directories
target_include_directories(offline_protocol_jni PRIVATE
    ${CMAKE_SOURCE_DIR}
)

