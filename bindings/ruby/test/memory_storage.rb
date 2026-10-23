# frozen_string_literal: true

# A thread-safe in-memory store, as a host writes one, behind both storage
# interfaces. They cannot be one object: the key material interface takes and
# returns an array of integers and the protocol-state one a binary string, and
# the generated code checks neither, so a mix-up fails inside the library.
class MemoryStorage
  def initialize
    @entries = {}
    @lock = Mutex.new
  end

  def size
    @lock.synchronize { @entries.size }
  end

  def put(type, id, bytes)
    @lock.synchronize { @entries[[type, id]] = bytes }
  end

  def get(type, id)
    @lock.synchronize { @entries[[type, id]] }
  end

  def remove(type, id)
    @lock.synchronize { @entries.delete([type, id]) }
  end

  def ids(type)
    @lock.synchronize { @entries.keys.select { |t, _| t == type }.map(&:last) }
  end

  # The protocol-state provider: bytes as a binary String.
  class State
    def initialize(store)
      @store = store
    end

    def store(type, id, data)
      @store.put(type, id, data.b)
    end

    def load(type, id)
      @store.get(type, id)&.b
    end

    def delete(type, id)
      @store.remove(type, id)
    end

    def list_keys(type)
      @store.ids(type)
    end
  end

  # The key material provider: bytes as an Array of Integers.
  class Mls
    def initialize(store)
      @store = store
    end

    def store(type, id, data)
      @store.put(type, id, data.pack("C*"))
    end

    def load(type, id)
      @store.get(type, id)&.bytes
    end

    def delete(type, id)
      @store.remove(type, id)
    end

    def list_keys(type)
      @store.ids(type)
    end
  end

  def state
    State.new(self)
  end

  def mls
    Mls.new(self)
  end
end
