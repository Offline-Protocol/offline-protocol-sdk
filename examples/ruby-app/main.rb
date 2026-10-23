# frozen_string_literal: true

# Two devices, Alice and Bob, in one Ruby process. They pair, establish an
# end-to-end encrypted session, exchange conversation signals, transfer a file,
# and talk in an MLS group.
#
# A real app moves bytes over a radio or a socket. Here the "network" is a loop
# that takes what one engine wants to send (internet_get_next_message) and hands
# it to the engine it is addressed to (internet_message_received), which is
# exactly what a transport does. Nothing else is simulated: the pairing, the MLS
# session and the encryption are the engine's own.

require "json"
require "securerandom"
require "tmpdir"
require "offline_protocol"

Sdk = OfflineProtocol

# The configuration this example needs. Only the fields the definition gives no
# default are named; the generated record keeps the rest (docs/bridges/ruby.md, R2).
def config(profile)
  Sdk::ProtocolConfig.new(
    app_id: "com.example.ruby", profile: profile,
    ble_enabled: false, wifi_direct_enabled: false, internet_enabled: true,
    reticulum_enabled: false, nostr_enabled: false, prefer_online: true,
    initial_ttl: 8, encryption_enabled: true, auto_key_exchange: true,
    # The pending queue: per peer, global, the SDK's 24 h lifetime, overflow.
    store_pending: true, max_pending_per_peer: 64, max_pending_global: 4096,
    pending_ttl_ms: 86_400_000, overflow_policy: Sdk::OverflowPolicy::DROP_OLDEST
  )
end

# One device: an engine, the name it pairs under, and the events it has seen.
class Device
  attr_reader :name, :engine, :address, :seen

  def initialize(name, storage)
    @name = name
    @engine = Sdk::OfflineProtocol.new(config(name))
    @seen = []

    # The engine's own encrypted file stores. An app with a keystore of its own
    # writes two storage providers and calls initialize_mls instead.
    mls = File.join(storage, "mls")
    state = File.join(storage, "state")
    Dir.mkdir(mls)
    Dir.mkdir(state)
    @engine.initialize_mls_with_file_stores(mls, state, SecureRandom.bytes(32))
    @address = @engine.local_address

    @engine.start
    # The internet slot is the carrier here, and it is "up".
    @engine.internet_status_changed(true)
  end

  # A sequence<u8> comes back from the library as signed bytes, -128 to 127, and
  # the engine refuses those on the way in, so mask them back to 0 to 255 before
  # handing them on: a defect in the generator (docs/bridges/ruby.md, R8).
  def key_package
    @engine.mls_get_or_create_key_package.key_package_data.map { |b| b & 0xFF }
  end

  # Reads the engine's events and answers the pairing ones, as an app does.
  def handle_events
    while (json = @engine.poll_event)
      event = JSON.parse(json)
      @seen << event
      case event["type"]
      when "connection_request_received"
        from = event["sender"]
        import_key_package(from, event["key_package"])
        @engine.accept_connection_request(from, @name, key_package)
        try_establish(from)
      when "connection_accepted"
        from = event["accepted_by"]
        import_key_package(from, event["key_package"])
        try_establish(from)
      end
    end
  end

  def saw?(type)
    @seen.any? { |e| e["type"] == type && (!block_given? || yield(e)) }
  end

  def in_group?(group_id)
    @engine.list_groups.include?(group_id)
  end

  def received?(content)
    saw?("message_received") { |e| e["content"] == content }
  end

  private

  # Best effort. With auto_key_exchange on, the engine finishes the session
  # itself once the peer's key package arrives over the wire, so a refusal for
  # want of one here only means "not yet". Both sides calling this is fine: when
  # both create a session, the engine keeps one by a tiebreak and the other side
  # adopts it.
  def try_establish(peer)
    @engine.establish_secure_session(peer)
  rescue Sdk::ProtocolError::SessionNotReady
    # The key package comes next.
  end

  def import_key_package(peer, key_package)
    @engine.mls_import_key_package(peer, key_package) if key_package.is_a?(Array) && !key_package.empty?
  end
end

# The network: whatever one device wants to send goes to the device it is
# addressed to.
def carry(devices)
  devices.each do |from|
    while (message = from.engine.internet_get_next_message)
      targets = devices.select { |to| to.address == message.recipient_id }
      targets.each { |to| to.engine.internet_message_received(from.address, message.data) }
      # Every frame taken gets an outcome, as a real socket bridge reports one
      # after its write: confirmed once carried, failed when there was nobody to
      # carry it to. A frame given neither waits out the transport's timeout
      # before it counts as lost.
      if targets.empty?
        from.engine.internet_send_failed(message.message_id)
      else
        from.engine.internet_confirm_sent(message.message_id)
      end
    end
  end
end

def until_done(devices, what)
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 20
  until yield
    raise "timed out waiting for: #{what}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline

    devices.each do |device|
      device.engine.process
      device.handle_events
    end
    carry(devices)
    sleep 0.05
  end
  puts "  ok: #{what}"
end

Dir.mktmpdir("offline-protocol-ruby-example") do |root|
  alice = Device.new("alice", Dir.mkdir(File.join(root, "alice")) && File.join(root, "alice"))
  bob = Device.new("bob", Dir.mkdir(File.join(root, "bob")) && File.join(root, "bob"))
  devices = [alice, bob]
  puts "alice #{alice.address}"
  puts "bob   #{bob.address}"

  begin
    # Pairing: Alice asks, Bob accepts, and each ends up holding the other's key
    # package, which is what the session is made from.
    alice.engine.send_connection_request(bob.address, alice.name, alice.key_package, nil)
    until_done(devices, "bob and alice are paired") do
      alice.engine.mls_has_session(bob.address) && bob.engine.mls_has_session(alice.address)
    end

    alice.engine.send_presence_update(bob.address, Sdk::PresenceStatus::ONLINE)
    until_done(devices, "bob sees alice online") do
      bob.saw?("presence_updated") { |e| e["peer_id"] == alice.address && e["status"].to_s.casecmp?("online") }
    end

    alice.engine.send_typing_indicator(bob.address, bob.address, true)
    until_done(devices, "bob sees alice typing") do
      bob.saw?("typing_indicator_received") { |e| e["sender"] == alice.address && e["is_typing"] == true }
    end

    greeting_id = alice.engine.send_message(bob.address, "hello bob", Sdk::MessagePriority::MEDIUM, nil)
    until_done(devices, "bob received \"hello bob\" (encrypted end to end)") { bob.received?("hello bob") }

    bob.engine.send_read_receipt(alice.address, [greeting_id])
    until_done(devices, "alice received a read receipt") do
      alice.saw?("read_receipt_received") { |e| e["message_ids"] == [greeting_id] }
    end

    # Reply by message ID so the recipient can thread the conversation.
    bob.engine.send_message_rich(
      alice.address, "hello alice",
      Sdk::SendMessageOptions.new(priority: Sdk::MessagePriority::HIGH, reply_to_msg: greeting_id,
                                  content_type: Sdk::ContentType::TEXT, app_id: "com.example.ruby")
    )
    until_done(devices, "alice received a threaded reply") do
      alice.saw?("message_received") { |e| e["content"] == "hello alice" && e["reply_to_msg"] == greeting_id }
    end

    # send_file chunks and encrypts the bytes; file_received carries the
    # reassembled data as base64 for the host to save or display. File data
    # crosses as an array of byte values (docs/bridges/ruby.md, R5).
    attachment = "Offline Protocol Ruby attachment\n"
    file_id = alice.engine.send_file(bob.address, attachment.bytes, "note.txt")
    until_done(devices, "bob received the original file bytes") do
      bob.saw?("file_received") do |e|
        e["file_id"] == file_id && e["file_name"] == "note.txt" &&
          e["file_data"].unpack1("m0") == attachment
      end
    end

    # MLS groups use the same carrier, but keep their own membership and
    # encryption state.
    group_id = alice.engine.create_group("Project team").group_id
    alice.engine.invite_to_group(group_id, bob.address)
    until_done(devices, "bob joined the MLS group") { bob.in_group?(group_id) }
    alice.engine.send_group_message(group_id, "team update", Sdk::MessagePriority::HIGH, nil)
    until_done(devices, "bob received the group message") do
      bob.saw?("group_message_received") { |e| e["group_id"] == group_id && e["content"] == "team update" }
    end

    puts "alice transport #{alice.engine.get_active_transports.inspect}"
    puts "group members #{alice.engine.get_group_info(group_id).members.inspect}"

    puts "done"
  ensure
    # The engines stay reachable until the process ends: a collected engine
    # that holds a callback leaks it (R3). These hold none. Closing the file
    # stores releases their lock files, which Windows will not delete while
    # they are open, so the temporary directory can be removed.
    devices.each do |device|
      device.engine.stop
      device.engine.close_file_stores
    end
  end
end
