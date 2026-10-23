# frozen_string_literal: true

# The gem a release publishes: the generated binding and the notices its licenses
# ask to travel with it, and no native code. The host supplies the library for
# its platform from the release's desktop archives (docs/bridges/ruby.md, R7).
#
# Build it with `rake build`, not `gem build`: the notices live at the root of
# the repository, and the task stages them next to this file first. Staged, not
# committed: a copy is a notice someone updates in one place.

Gem::Specification.new do |spec|
  spec.name = "offline_protocol"
  # Overridden by the release with OFFLINE_PROTOCOL_VERSION, as the Java build's
  # VERSION_NAME is. A prerelease such as 0.31.0-rc.1 becomes 0.31.0.pre.rc.1.
  spec.version = ENV.fetch("OFFLINE_PROTOCOL_VERSION", "0.0.0")
  spec.summary = "Offline-first messaging with MLS end-to-end encryption, for Ruby"
  spec.description = "BLE mesh and internet relay transports, with MLS end-to-end " \
                     "encryption (RFC 9420) applied automatically. Needs the native " \
                     "library for the platform, which this gem does not contain."
  spec.authors = ["Offline Protocol, Inc."]
  spec.homepage = "https://github.com/Offline-Protocol/offline-protocol-sdk"
  spec.license = "AGPL-3.0-only"
  spec.metadata = {
    "source_code_uri" => "https://github.com/Offline-Protocol/offline-protocol-sdk/tree/main/bindings/ruby",
    "documentation_uri" => "https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/bridges/ruby.md",
    "changelog_uri" => "https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/CHANGELOG.md",
    "rubygems_mfa_required" => "true"
  }

  # The oldest Ruby CI runs the consumer check on.
  spec.required_ruby_version = ">= 3.3"
  spec.add_dependency "ffi", "~> 1.17"

  # The binding's own two files, then the notices `rake build` stages.
  spec.files = %w[lib/offline_protocol.rb README.md] +
               %w[LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md]
  spec.require_paths = ["lib"]
end
