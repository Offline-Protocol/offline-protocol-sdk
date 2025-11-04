cmake_minimum_required(VERSION 3.22.1)

project(offline_protocol_jni)

# Set C++ standard
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Find required packages
find_library(log-lib log)
find_library(jnigraphics-lib jnigraphics)

# Include directories
# The header file should be copied to android/src/main/cpp/ during build
include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}
)

# Source files
set(SOURCES
    offline_protocol_jni.cpp
)

# Create shared library
add_library(
    offline_protocol_ffi
    SHARED
    ${SOURCES}
)

# Link against the pre-built offline_protocol_ffi library
# The library should be in jniLibs directory for each architecture
# The .so file will be loaded at runtime via System.loadLibrary()
target_link_libraries(
    offline_protocol_ffi
    ${log-lib}
    ${jnigraphics-lib}
)

# Copy header file if needed
# The header should be available from the FFI crate

