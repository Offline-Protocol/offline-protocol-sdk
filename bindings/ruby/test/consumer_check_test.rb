# frozen_string_literal: true

require_relative "test_helper"

# The Ruby program that proves the generated binding, the library and the
# interface definition agree: the counterpart of bindings/c/consumer-check and
# of the Java ConsumerCheckTest. It loads the real library, whose FFI checksums
# the generated code verifies before the first call, so a binding generated from
# a different definition than the library fails every test here, and so does a
# config that names the wrong library.
class ConsumerCheckTest < Minitest::Test
  STORE_KEY = (0..31).to_a.pack("C*").freeze

  # Only the fields the definition gives no default; the generated record keeps
  # the rest, so a Ruby caller leaves them out as a Python caller does.
  def config
    Sdk::ProtocolConfig.new(
      app_id: "com.example.ruby", profile: "ruby-consumer",
      ble_enabled: false, wifi_direct_enabled: false, internet_enabled: true,
      reticulum_enabled: false, nostr_enabled: false, prefer_online: false,
      initial_ttl: 8, encryption_enabled: true, auto_key_exchange: true,
      # The pending queue: per peer, global, the SDK's 24 h lifetime, overflow.
      store_pending: true, max_pending_per_peer: 64, max_pending_global: 4096,
      pending_ttl_ms: 86_400_000, overflow_policy: Sdk::OverflowPolicy::DROP_OLDEST
    )
  end

  # Every engine built here lives until the process ends. The generated class
  # frees its Rust object from a GC finalizer, and releasing an event callback
  # takes a lock that Ruby forbids there, so a collected engine that holds a
  # callback leaks the handle and prints a warning (docs/bridges/ruby.md, R3).
  # A host keeps its one engine for the life of the process and never meets it.
  ENGINES = []

  def engine
    Sdk::OfflineProtocol.new(config).tap { |e| ENGINES << e }
  end

  def test_an_instance_less_function_needs_no_engine
    assert_equal "off1q9nxs74dlp3t6amv3lqchr5l3csq39c5s5grq3yh", Sdk.derive_address([0] * 32)
  end

  def test_a_rfc_8032_key_derives_the_pinned_address
    key = ["d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"].pack("H*").bytes
    assert_equal "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn", Sdk.derive_address(key)
  end

  def test_the_definitions_defaults_are_kept_on_the_record
    c = config
    assert c.require_encryption
    assert_equal 256, c.max_group_members
    assert c.group_relay_enabled
    refute c.group_enforce_admin_commits
    refute c.nostr_username_discovery_enabled
    assert c.data_enabled
    assert_nil c.mesh_relay
    assert_nil c.custody
  end

  def test_an_engine_answers_and_events_reach_both_the_callback_and_the_poll_queue
    e = engine
    assert_equal Sdk::ProtocolState::STOPPED, e.get_state

    heard = []
    thread = nil
    listener = Object.new
    listener.define_singleton_method(:on_event) do |json|
      thread = Thread.current
      heard << json
    end
    e.set_event_callback(listener)
    e.emit_test_event

    assert_equal 1, heard.size
    assert_same Thread.current, thread, "the event runs on the calling thread"
    assert_equal heard.first, e.poll_event
    assert_nil e.poll_event
  end

  def test_an_engine_refusal_is_a_typed_exception
    e = engine
    e.start
    # No storage yet, so there is no session to encrypt under.
    assert_raises(Sdk::ProtocolError::EncryptFailed) do
      e.send_message("off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s", "hi",
                     Sdk::MessagePriority::MEDIUM, nil)
    end
    e.stop
  end

  def test_a_ruby_storage_provider_passes_the_conformance_suite
    report = Sdk.run_storage_conformance(MemoryStorage.new.state)
    assert_includes report, '"failures":[]'
  end

  def test_two_ruby_providers_give_the_engine_an_address
    store = MemoryStorage.new
    e = engine
    e.initialize_mls(store.mls, store.state)
    assert e.local_address.start_with?("off1")
    assert_operator store.size, :>, 0, "nothing reached the providers"
    e.start
    e.stop
  end

  # The key material provider's store receives signed values (R8) and keeps
  # them through pack("C*"), which wraps them back to bytes. Only a reload of
  # sealed state proves that: the record key that seals it is random bytes and
  # goes through the signed read, where the identity key pair is stored as text
  # and never does. A store that kept the wrong bytes reopens a flushed document
  # as absent, without an error, and passes every other test here.
  def test_a_document_sealed_through_ruby_providers_survives_a_reload
    store = MemoryStorage.new
    title = '{"kind":"text","value":"hello"}'
    first = engine
    first.initialize_mls(store.mls, store.state)
    docs = Sdk::DataStore.new(first)
    docs.create_doc("notes", "n1")
    docs.map_set("notes", "n1", "fields", "title", title)
    docs.flush("notes", "n1")

    second = engine
    second.initialize_mls(store.mls, store.state)
    assert_equal first.local_address, second.local_address, "the identity did not survive a reload"
    assert_equal title, Sdk::DataStore.new(second).map_get_json("notes", "n1", "fields", "title"),
                 "a sealed document did not survive a reload through the Ruby providers"
  end

  def test_the_built_in_file_stores_need_no_provider
    Dir.mktmpdir do |dir|
      mls = File.join(dir, "mls")
      state = File.join(dir, "state")
      Dir.mkdir(mls)
      Dir.mkdir(state)
      e = engine
      e.initialize_mls_with_file_stores(mls, state, STORE_KEY)
      assert e.local_address.start_with?("off1")
      e.close_file_stores
    end
  end

  # The generator reads a u8 with a signed unpack, so bytes come back from the
  # library as -128 to 127, and its own range check refuses them on the way back
  # in (docs/bridges/ruby.md, R8). A key package is a few hundred random-looking
  # bytes, so it holds one above 127. When this fails, the generator was fixed:
  # drop the mask here, in the example and in R8.
  def test_a_byte_sequence_comes_back_signed_and_masks_back
    Dir.mktmpdir do |dir|
      mls = File.join(dir, "mls")
      state = File.join(dir, "state")
      Dir.mkdir(mls)
      Dir.mkdir(state)
      e = engine
      e.initialize_mls_with_file_stores(mls, state, STORE_KEY)
      e.start
      peer = "off1qyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyr4s29s"

      signed = e.mls_get_or_create_key_package.key_package_data
      assert_operator signed.min, :<, 0, "u8 now reads unsigned: the generator was fixed, retire R8"
      assert_raises(RangeError) { e.send_connection_request(peer, "ruby", signed, nil) }

      masked = signed.map { |b| b & 0xFF }
      assert masked.all? { |b| b.between?(0, 255) }
      refute_empty e.send_connection_request(peer, "ruby", masked, nil)
      e.stop
      e.close_file_stores
    end
  end

  def test_a_provider_that_raises_is_reported_as_a_storage_failure
    failing = Object.new
    def failing.store(_type, _id, _data) = raise(Sdk::MlsStorageError::StoreFailed, "disk full")
    def failing.load(_type, _id) = nil
    def failing.delete(_type, _id) = nil
    def failing.list_keys(_type) = []

    report = Sdk.run_storage_conformance(failing)
    refute_includes report, '"failures":[]', "a provider that cannot store must not pass: #{report}"
    # The exception crossed back into the engine as its storage error, not as a crash.
    assert_includes report, "Failed to store protocol state"
  end
end
