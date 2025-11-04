require "json"

package = JSON.parse(File.read(File.join(__dir__, "../package.json")))

Pod::Spec.new do |s|
  s.name         = "OfflineProtocol"
  s.version      = package["version"]
  s.summary      = "React Native bindings for the Offline Protocol SDK"
  s.description  = <<-DESC
                  A high-performance, cross-platform SDK for offline-first messaging with intelligent transport switching.
                  DESC
  s.homepage     = "https://github.com/offline-protocol/sdk"
  s.license      = "MIT OR Apache-2.0"
  s.author       = { "Offline Protocol Contributors" => "contributors@offlineprotocol.org" }
  s.platforms    = { :ios => "12.0" }
  s.source       = { :git => "https://github.com/offline-protocol/sdk.git", :tag => "#{s.version}" }

  s.source_files = "*.{h,m,swift}"
  s.requires_arc = true

  # Link against the static library
  s.vendored_libraries = "liboffline_protocol_ffi.a"
  
  # Include the header file
  s.public_header_files = "offline_protocol.h"
  s.preserve_paths = "offline_protocol.h"

  # Search paths
  s.xcconfig = {
    "HEADER_SEARCH_PATHS" => "$(PODS_TARGET_SRCROOT)",
    "LIBRARY_SEARCH_PATHS" => "$(PODS_TARGET_SRCROOT)"
  }

  # React Native dependency
  s.dependency "React-Core"

  # Swift version
  s.swift_version = "5.0"
end

