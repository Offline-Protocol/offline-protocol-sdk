require 'json'

package = JSON.parse(File.read(File.join(__dir__, 'package.json')))

Pod::Spec.new do |s|
  s.name         = "OfflineProtocol"
  s.version      = package['version']
  s.summary      = package['description']
  s.homepage     = "https://github.com/offline-protocol/sdk"
  s.license      = package['license']
  s.authors      = { "Offline Protocol" => "contributors@offlineprotocol.org" }
  s.platforms    = { :ios => "12.0" }
  s.source       = { :git => "https://github.com/offline-protocol/sdk.git", :tag => "#{s.version}" }

  s.source_files = "ios/**/*.{h,m,mm,swift}"
  s.vendored_libraries = "ios/liboffline_protocol.a"
  
  s.dependency "React-Core"

  s.swift_version = '5.0'
end

