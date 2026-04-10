cmake_minimum_required(VERSION 3.18)

project(offline_protocol_jni)

# Find React Native
set(REACT_NATIVE_DIR ${CMAKE_SOURCE_DIR}/../../../../../node_modules/react-native)
set(BUILD_DIR ${CMAKE_SOURCE_DIR}/build)

# Path to pre-built Rust library
set(RUST_LIB_PATH ${CMAKE_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/liboffline_protocol_ffi.so)

# Add JNI wrapper
add_library(offline_protocol_jni SHARED
    offline_protocol_jni.cpp
)

# Copy the Rust library to the output directory so it gets packaged in the APK
# Do this BEFORE building the JNI library
add_custom_command(
    OUTPUT ${CMAKE_CURRENT_BINARY_DIR}/liboffline_protocol_ffi.so
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
        ${RUST_LIB_PATH}
        ${CMAKE_CURRENT_BINARY_DIR}/liboffline_protocol_ffi.so
    DEPENDS ${RUST_LIB_PATH}
    COMMENT "Copying liboffline_protocol_ffi.so to build directory"
)

# Create a custom target that depends on the copied library
add_custom_target(copy_ffi_lib ALL
    DEPENDS ${CMAKE_CURRENT_BINARY_DIR}/liboffline_protocol_ffi.so
)

# Make sure the copy happens before building the JNI library
add_dependencies(offline_protocol_jni copy_ffi_lib)

# Import the Rust library - use the copied version in the build directory
add_library(offline_protocol_ffi SHARED IMPORTED)
set_target_properties(offline_protocol_ffi PROPERTIES
    IMPORTED_LOCATION ${CMAKE_CURRENT_BINARY_DIR}/liboffline_protocol_ffi.so
    IMPORTED_NO_SONAME TRUE
)

# Copy to the final output directory for APK packaging
add_custom_command(TARGET offline_protocol_jni POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
        ${CMAKE_CURRENT_BINARY_DIR}/liboffline_protocol_ffi.so
        $<TARGET_FILE_DIR:offline_protocol_jni>/liboffline_protocol_ffi.so
    COMMENT "Copying liboffline_protocol_ffi.so to output directory"
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

