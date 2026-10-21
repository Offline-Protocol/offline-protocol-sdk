require 'json'

# Shared React Native iOS bridge sources (ADR 0025). CocoaPods PathList walks the
# pod root with Dir.glob and does not follow directory symlinks, so prepare_command
# rsyncs the RN tree into BridgeRN/ (gitignored; refreshed on each pod install).
rn_ios = 'BridgeRN'
plugin_ios = File.dirname(File.realpath(__FILE__))

# PR3: PR2 Internet stack + BLE, peer-stream (Wi‑Fi Direct slot), event replay,
# foreground reconnect. Nostr / Reticulum stay on React Native until a later PR
# expands the compile surface.
# Keep `pr3_bridge_swift` aligned with android/build.gradle `pr3KotlinIncludes`
# and KotlinCompile excludes (Rust guards pin both).
pr3_bridge_swift = [
  'ProtocolConfigParser.swift',
  'InitialRuntimeConfigApplier.swift',
  'InternetTransportBootstrap.swift',
  'EncryptionConfigReader.swift',
  'MeshRelayConfigReader.swift',
  'CustodyConfigReader.swift',
  'MlsSecureStorage.swift',
  'ProtocolStateStorage.swift',
  'StorageNamespace.swift',
  'LegacyStoreAdoption.swift',
  'TransportManager.swift',
  'InternetManager.swift',
  'RelayControlOpTranslator.swift',
  'RelayAnswerPrefixes.swift',
  'RelayGroupSnapshotBridge.swift',
  'RelayRateLimiter.swift',
  'RelayTimestamps.swift',
  'LegacyRelayMessage.swift',
  'AddressDeclarationPolicy.swift',
  'ForcedPresenceCheckQueue.swift',
  'RecipientInFlightTracker.swift',
  'SupersededLatchPolicy.swift',
  'GatewayAttachPolicy.swift',
  'GatewayVerdictTracker.swift',
  'PeerIdentityBinding.swift',
  'MonotonicClock.swift',
  'SocketGenerationTracker.swift',
  'WriteStallWatchdog.swift',
  'ProtocolErrorBridge.swift',
  'PresenceWatchPolicy.swift',
  'EventReplayRing.swift',
  'ForegroundReconnectPolicy.swift',
  'BleManager.swift',
  'WifiDirectManager.swift',
  'BleAppTag.swift',
  'BleDensityPolicy.swift',
  'BleHelloPolicy.swift',
  'BleMessageNotificationPolicy.swift',
  'BleServiceInstanceSelection.swift',
  'InboundFragmentBuffer.swift',
  'OutboundFragmentQueue.swift',
  'PeerStreamFraming.swift',
  'PeerStreamSession.swift',
  'PeripheralRestorationAgeOutPolicy.swift',
].map { |name| File.join(rn_ios, name) } + [
  File.join(rn_ios, 'ble/**/*.swift'),
  File.join(rn_ios, 'mesh/**/*.swift'),
]

# React Native–only; Flutter uses EventReplayRing and shared transport stack above.
rn_only_swift = [
  'BridgeRN/OfflineProtocolModule.swift',
  'BridgeRN/OfflineProtocolModule.m',
  'BridgeRN/NostrManager.swift',
  'BridgeRN/NostrQueryTracker.swift',
  'BridgeRN/ReticulumManager.swift',
  'BridgeRN/InboundEventBuffer.swift',
]

Pod::Spec.new do |s|
  s.name             = 'offline_protocol'
  s.version          = '0.29.0'
  s.summary          = 'Flutter bindings for the Offline Protocol SDK'
  s.description      = 'Offline-first mesh messaging with MLS encryption (Flutter plugin).'
  s.homepage         = 'https://github.com/Offline-Protocol/offline-protocol-sdk'
  s.license          = { :type => 'AGPL-3.0-only' }
  s.author           = { 'Offline Protocol, Inc.' => 'support@offlineprotocol.com' }
  s.source           = { :path => '.' }
  s.platform         = :ios, '13.0'
  s.swift_version    = '5.5'

  sync_script = File.join(plugin_ios, 'sync_bridge_rn.sh')
  s.prepare_command = "bash #{sync_script.shellescape}"

  # pr3_bridge_swift names the compile surface for Rust guards; CocoaPods uses globs.
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

  s.vendored_frameworks = File.join(rn_ios, 'libs/offline_protocol_uniffi.xcframework')

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
