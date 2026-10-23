# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require "offline_protocol"
require_relative "memory_storage"

# The generated module is OfflineProtocol and so is the engine class inside it:
# the generator offers no way to rename the module.
Sdk = OfflineProtocol
