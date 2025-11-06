require 'json'

package = JSON.parse(File.read(File.join(__dir__, '../package.json')))

Pod::Spec.new do |s|
  s.name         = "OfflineProtocol"
  s.version      = package['version']
  s.summary      = package['description']
  s.homepage     = package['homepage']
  s.license      = package['license']
  s.authors      = package['author']

  s.platforms    = { :ios => "12.0" }
  s.source       = { :git => package['repository']['url'], :tag => "v#{s.version}" }

  s.source_files = "*.{h,m,swift}"
  s.public_header_files = "*.h"
  s.static_framework = true

  # Pre-built Rust library
  s.vendored_libraries = "libs/liboffline_protocol_ffi.a"

  # System frameworks
  s.frameworks = "Foundation"

  s.dependency "React-Core"

  # Swift support
  s.swift_version = "5.0"

  # Allow non-modular includes (required for React Native headers)
  s.pod_target_xcconfig = {
    'CLANG_ALLOW_NON_MODULAR_INCLUDES_IN_FRAMEWORK_MODULES' => 'YES',
    'DEFINES_MODULE' => 'YES'
  }

  s.user_target_xcconfig = {
    'CLANG_ALLOW_NON_MODULAR_INCLUDES_IN_FRAMEWORK_MODULES' => 'YES'
  }
end

