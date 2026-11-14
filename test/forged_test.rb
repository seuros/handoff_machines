# frozen_string_literal: true

require 'minitest/autorun'
require 'handoff_machines'
require_relative 'support'

# Sealing stops accidents; the native side is the authority. A value forged
# past `initialize` raises TypeError or RangeError wherever it is read, and
# never reaches the core.
class ForgedTest < Minitest::Test
  include Forge

  H = HandoffMachines
  LIMITS = H::Limits.new(5_000_000_000, 2)
  VALUES = [H::FencingToken, H::Limits, H::Ticket, H::Recorded, H::Duplicate, H::Replica, H::Drain, H::Capture,
            H::Install, H::Fence, H::Activate, H::Resume, H::Order, H::Status].freeze

  def tok(raw) = H::FencingToken.from_raw(raw)
  def snapshot(handoff) = [handoff.status(9), handoff.orders(0, 32), handoff.generation, handoff.active]

  def begun
    handoff = H::Handoff.new(LIMITS)
    [handoff, handoff.start(9, 1, 2, 10, tok(5), 0)]
  end

  def test_every_value_class_is_sealed
    VALUES.each do |value|
      assert_raises(NoMethodError) { value.allocate }
      assert_raises(TypeError) { Class.new(value) }
    end
  end

  # A named subclass still raises, but Ruby has bound the constant first: it
  # leaks. That is why sealing is not the authority.
  def test_a_named_subclass_raises_but_its_constant_leaks
    leaked(:LeakedTicket, H::Ticket)
    assert ForgedTest.const_defined?(:LeakedTicket, false)
    assert_operator ForgedTest::LeakedTicket, :<, H::Ticket
  end

  # A named subclass of `parent`: `class` raises TypeError, yet the constant
  # is bound. Given a `Data#initialize` bound past the check, it builds
  # values holding anything.
  def leaked(name, parent)
    unless ForgedTest.const_defined?(name, false)
      source = "class ForgedTest::#{name} < #{parent}; end"
      assert_raises(TypeError) { eval(source) } # rubocop:disable Security/Eval
    end
    klass = ForgedTest.const_get(name)
    klass.class_eval do
      define_method(:initialize) { |**fields| Data.instance_method(:initialize).bind_call(self, **fields) }
    end
    klass
  end

  def test_a_subclass_instance_is_refused_whatever_it_holds
    handoff, ticket = begun
    before = snapshot(handoff)
    [[9, 0], [9, 1]].each do |partition, generation|
      forged = leaked(:LeakedTicket, H::Ticket).new(partition:, generation:)
      assert_raises(TypeError) { handoff.quiesced(forged, 1, 12, 1) }
      assert_raises(TypeError) { handoff.outstanding(forged, 0, 4) }
      assert_raises(TypeError) { H::Order.new(forged, H::Replica.new(1), :quiesce) }
      assert_raises(TypeError) { forged.scope }
    end
    token = leaked(:LeakedToken, H::FencingToken).new(raw: 0)
    assert_raises(TypeError) { handoff.start(3, 3, 4, 0, token, 1) }
    assert_raises(TypeError) { handoff.fenced(ticket, token, 1) }
    assert_raises(TypeError) { H::Fence.new(token) }
    limits = leaked(:LeakedLimits, H::Limits).new(phase_budget: 1, per_replica: 0)
    assert_raises(TypeError) { H::Handoff.new(limits) }
    replica = leaked(:LeakedReplica, H::Replica).new(id: 1)
    assert_raises(TypeError) { H::Order.new(ticket, replica, :quiesce) }
    drain = leaked(:LeakedDrain, H::Drain).new(until: 12)
    assert_raises(TypeError) { H::Order.new(ticket, H::Replica.new(1), drain) }
    assert_equal before, snapshot(handoff)
  end

  def test_a_subclass_instance_is_refused_like_any_other_value
    handoff, ticket = begun
    forged = leaked(:LeakedTicket, H::Ticket).new(partition: 9, generation: 1)
    error = assert_raises(TypeError) { handoff.outstanding(forged, 0, 4) }
    assert_equal 'expected a HandoffMachines::Ticket', error.message
    token = leaked(:LeakedToken, H::FencingToken).new(raw: 5)
    error = assert_raises(TypeError) { handoff.fenced(ticket, token, 1) }
    assert_equal 'expected a HandoffMachines::FencingToken or a value with #raw', error.message
    replica = leaked(:LeakedReplica, H::Replica).new(id: 1)
    error = assert_raises(TypeError) { H::Order.new(ticket, replica, :quiesce) }
    assert_equal 'a recipient is :authority or a HandoffMachines::Replica', error.message
  end

  def test_a_marshalled_value_is_validated_again
    handoff, ticket = begun
    before = snapshot(handoff)
    unissued = forge(H::Ticket.new(9, 1), "i\x06".b, "i\x00".b)
    assert_equal 0, unissued.generation
    assert_raises(RangeError) { handoff.quiesced(unissued, 1, 12, 1) }
    assert_raises(RangeError) { H::Order.new(unissued, H::Replica.new(1), :quiesce) }
    token = forge(H::FencingToken.new(1), "i\x06".b, "i\x00".b)
    assert_raises(RangeError) { handoff.start(3, 3, 4, 0, token, 1) }
    assert_raises(RangeError) { handoff.abort(ticket, token, 1) }
    limits = forge(H::Limits.new(77, 1), "i\x06".b, "i\x00".b)
    assert_equal 0, limits.per_replica
    assert_raises(RangeError) { H::Handoff.new(limits) }
    fence = forge(H::Fence.new(tok(1)), "i\x06".b, "i\x00".b)
    assert_raises(RangeError) { H::Order.new(ticket, :authority, fence) }
    assert_equal before, snapshot(handoff)
  end

  # A singleton class on the instance itself does not change what it is.
  def test_a_value_with_its_own_singleton_class_is_still_this_gems
    handoff, ticket = begun
    ticket.singleton_class
    assert_equal H::Recorded.new(:drain), handoff.quiesced(ticket, 1, 12, 1)
    assert_instance_of H::Handoff, H::Handoff.new(H::Limits.new(1, 1).tap(&:singleton_class))
  end

  # This gem's values are read from the struct, never through their readers:
  # a redefined reader is ignored (a sibling's token is still read by its).
  def test_a_redefined_reader_is_ignored
    handoff, ticket = begun
    order = H::Order.new(ticket, H::Replica.new(1), H::Drain.new(12))
    readers = { H::FencingToken => %i[raw], H::Limits => %i[phase_budget per_replica],
                H::Ticket => %i[partition generation], H::Replica => %i[id], H::Drain => %i[until] }
    originals = readers.flat_map { |klass, names| names.map { [klass, it, klass.instance_method(it)] } }
    begin
      originals.each { |klass, name, _| klass.define_method(name) { raise "#{name} reader ran" } }
      quiesced = handoff.quiesced(ticket, 1, 12, 1)
      other = H::Handoff.new(LIMITS)
      started = other.start(9, 1, 2, 10, tok(5), 0)
      rebuilt = H::Order.new(order.ticket, order.to, order.action)
    ensure
      originals.each { |klass, name, reader| klass.define_method(name, reader) }
    end
    assert_equal H::Recorded.new(:drain), quiesced
    assert_equal ticket, started
    assert_equal order, rebuilt
  end
end
