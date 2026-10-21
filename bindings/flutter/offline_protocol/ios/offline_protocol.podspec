require 'json'

# Shared React Native iOS bridge sources (ADR 0025). CocoaPods PathList walks the
# pod root with Dir.glob and does not follow directory symlinks, so prepare_command
# rsyncs the RN tree into BridgeRN/ (gitignored; refreshed on each pod install).
rn_ios = 'BridgeRN'
plugin_ios = File.dirname(File.realpath(__FILE__))

# Values the React Native package declares are read from it, never written
# again here (ADR 0025 decision 5): a copy is right on the day it is made and
# drifts with nothing failing. The version comes from package.json, which the
# release gate keeps in step with the tag; the deployment target comes from the
# line in MeshSdk.podspec that declares it, which has to be there exactly once.
rn_root = File.expand_path('../../../react-native', plugin_ios)
rn_package = JSON.parse(File.read(File.join(rn_root, 'package.json')))
rn_targets = File.read(File.join(rn_root, 'MeshSdk.podspec'))
  .scan(/s\.platforms\s*=\s*\{\s*:ios\s*=>\s*"([0-9.]+)"\s*\}/).flatten
unless rn_targets.length == 1
  raise "MeshSdk.podspec must declare the iOS deployment target exactly once (found #{rn_targets.length})"
end
ios_deployment_target = rn_targets.first

# The compile surface is every shared bridge source except the ones named here
# (ADR 0025 decision 3): a list of what to leave out, never a list of what to
# take, so a new shared source reaches this pod without being registered.
# These need React, or are transports the plugin does not host yet. Keep in
# step with the rsync excludes in sync_bridge_rn.sh; a Rust guard
# (flutter_ios_exclude_lists_agree_and_name_real_files) holds both to the
# sources that exist.
rn_only_swift = [
  'BridgeRN/OfflineProtocolModule.swift',
  'BridgeRN/OfflineProtocolModule.m',
  'BridgeRN/ProtocolEngineHostBridgeReactNative.swift',
]

Pod::Spec.new do |s|
  s.name             = 'offline_protocol'
  s.version          = rn_package['version']
  s.summary          = 'Flutter bindings for the Offline Protocol SDK'
  s.description      = 'Offline-first mesh messaging with MLS encryption (Flutter plugin).'
  s.homepage         = 'https://github.com/Offline-Protocol/offline-protocol-sdk'
  s.license          = { :type => 'AGPL-3.0-only' }
  s.author           = { 'Offline Protocol, Inc.' => 'support@offlineprotocol.com' }
  s.source           = { :path => '.' }
  s.platform         = :ios, ios_deployment_target
  s.swift_version    = '5.5'

  sync_script = File.join(plugin_ios, 'sync_bridge_rn.sh')
  s.prepare_command = "bash #{sync_script.shellescape}"

  s.source_files = [
    'Classes/**/*.swift',
    'BridgeRN/**/*.swift',
    'BridgeRN/Generated/*.h',
  ]
  s.exclude_files = rn_only_swift

  s.resource_bundles = {
    'OfflineProtocolPrivacy' => [File.join(rn_ios, 'PrivacyInfo.xcprivacy')],
  }

  s.preserve_paths = [
    File.join(rn_ios, 'Generated/offline_protocolFFI.modulemap'),
  ]

  staged_xcf = File.join(plugin_ios, 'Frameworks/offline_protocol_uniffi.xcframework')
  rn_xcf = File.join(rn_ios, 'libs/offline_protocol_uniffi.xcframework')
  s.vendored_frameworks = File.directory?(staged_xcf) ? staged_xcf : rn_xcf

  s.public_header_files = [
    File.join(rn_ios, 'Generated/offline_protocolFFI.h'),
  ]

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_INCLUDE_PATHS' => '$(PODS_TARGET_SRCROOT)/BridgeRN/Generated',
    'HEADER_SEARCH_PATHS' => '$(inherited) $(PODS_TARGET_SRCROOT)/BridgeRN/Generated',
  }

  s.frameworks = 'Foundation', 'Security', 'Network', 'CoreBluetooth'
  s.dependency 'Flutter'
end
