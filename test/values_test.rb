# frozen_string_literal: true

require 'minitest/autorun'
require 'handoff_machines'

# Every value is valid or does not exist, holds this gem's classes, and a call
# that raises has changed nothing.
class ValuesTest < Minitest::Test
  H = HandoffMachines
  SECOND = 1_000_000_000
  LIMITS = H::Limits.new(5 * SECOND, 2)
  SiblingToken = Data.define(:raw)

  # A token whose reader raises.
  class Exploding
    def raw = raise(IOError, 'boom')
  end

  def tok(raw) = H::FencingToken.from_raw(raw)
  def snapshot(handoff) = [handoff.status(9), handoff.orders(0, 32), handoff.generation, handoff.active]

  def begun
    handoff = H::Handoff.new(LIMITS)
    [handoff, handoff.start(9, 1, 2, 10, tok(5), 0)]
  end

  def test_limits_and_tickets
    assert_raises(RangeError) { H::Limits.new(SECOND, 0) }
    assert_raises(RangeError) { H::Limits.new(-1, 1) }
    assert_raises(TypeError) { H::Limits.new(1.0, 1) }
    assert_equal 0, H::Limits.new(0, 1).phase_budget
    assert_raises(RangeError) { H::Ticket.new(9, 0) }
    assert_raises(RangeError) { H::Ticket.new(9, 3 << 48) }
    assert_raises(TypeError) { H::Ticket.new('9', 1) }
    assert_raises(ArgumentError) { H::Ticket.new(9) }
    assert_equal 3, H::Ticket.new(9, (3 << 48) | 1).scope
  end

  def test_acknowledgement_answers
    assert_raises(RangeError) { H::Recorded.new(:quiesce) }
    assert_raises(RangeError) { H::Recorded.new(:nope) }
    assert_raises(TypeError) { H::Recorded.new('drain') }
    %i[quiesce complete aborted].each { |phase| assert_raises(RangeError) { H::Duplicate.new(phase) } }
    assert_predicate H::Duplicate.new(:rollback), :duplicate?
    assert_equal :complete, H::Recorded.new(:complete).phase
  end

  def test_actions_and_recipients
    [[H::Replica, -1, 'id'], [H::Drain, 2**64, 'until'], [H::Capture, -1, 'at'],
     [H::Install, 2**64, 'checkpoint']].each do |klass, bad, field|
      assert_equal "#{field} must be in 0..=18446744073709551615", assert_raises(RangeError) { klass.new(bad) }.message
      assert_equal "#{field} must be an Integer", assert_raises(TypeError) { klass.new(1.0) }.message
      assert_raises(ArgumentError) { klass.new }
    end
    assert_equal 12, H::Drain.new(until: 12).until
  end

  def test_tokens_are_converted_on_the_way_in
    sibling = SiblingToken.new(6)
    [H::Fence, H::Activate, H::Resume].each do |klass|
      kept = klass.new(sibling).to_h.values.first
      assert_instance_of H::FencingToken, kept
      assert_equal klass.new(tok(6)), klass.new(sibling)
      assert_raises(TypeError) { klass.new(6) }
      assert_raises(RangeError) { klass.new(SiblingToken.new(0)) }
    end
    assert_nil H::Resume.new(nil).token
    assert_raises(TypeError) { H::Fence.new(nil) }
  end

  def test_orders
    ticket = H::Ticket.new(9, 1)
    one = H::Replica.new(1)
    assert_equal :authority, H::Order.new(ticket, :authority, H::Fence.new(SiblingToken.new(5))).to
    assert_raises(RangeError) { H::Order.new(ticket, :authority, :quiesce) }
    assert_raises(RangeError) { H::Order.new(ticket, one, H::Fence.new(tok(5))) }
    assert_raises(RangeError) { H::Order.new(ticket, :nobody, :quiesce) }
    %i[drain capture install fence activate resume].each do |name|
      assert_raises(RangeError) { H::Order.new(ticket, one, name) }
    end
    assert_raises(RangeError) { H::Order.new(ticket, one, :nope) }
    assert_raises(TypeError) { H::Order.new(ticket, 1, :quiesce) }
    assert_raises(TypeError) { H::Order.new([9, 1], one, :quiesce) }
    assert_raises(TypeError) { H::Order.new(ticket, one, 'quiesce') }
    assert_raises(TypeError) { H::Order.new(ticket, one, Object.new) }
  end

  # A recipient or an action is one argument with one message, whichever
  # half refused it: RangeError for a Symbol, TypeError for anything else.
  def test_a_refused_recipient_or_action_names_what_it_takes
    ticket = H::Ticket.new(9, 1)
    one = H::Replica.new(1)
    recipient = 'a recipient is :authority or a HandoffMachines::Replica'
    assert_equal recipient, assert_raises(RangeError) { H::Order.new(ticket, :replica, :quiesce) }.message
    assert_equal recipient, assert_raises(TypeError) { H::Order.new(ticket, 1, :quiesce) }.message
    action = 'an action is :quiesce, :discard or an action Data (exactly its class)'
    assert_equal action, assert_raises(RangeError) { H::Order.new(ticket, one, :drain) }.message
    assert_equal action, assert_raises(TypeError) { H::Order.new(ticket, one, 'quiesce') }.message
    assert_equal action, assert_raises(TypeError) { H::Order.new(ticket, one, one) }.message
    error = assert_raises(TypeError) { H::Fence.new(6) }
    assert_equal 'expected a HandoffMachines::FencingToken or a value with #raw', error.message
    error = assert_raises(TypeError) { H::Order.new([9, 1], one, :quiesce) }
    assert_equal 'expected a HandoffMachines::Ticket', error.message
    assert_equal 'expected a HandoffMachines::Limits', assert_raises(TypeError) { H::Handoff.new([1, 1]) }.message
  end

  def test_statuses
    handoff, ticket = begun
    status = handoff.status(9)
    assert_equal status, H::Status.new(*status.to_h.values)
    assert_raises(RangeError) { H::Status.new(**status.to_h, phase: :complete) }
    assert_raises(RangeError) { H::Status.new(**status.to_h, target: 1) }
    assert_raises(RangeError) { H::Status.new(**status.to_h, until: 12, checkpoint: 77) }
    assert_raises(RangeError) { H::Status.new(**status.to_h, phase: :nope) }
    assert_raises(TypeError) { H::Status.new(**status.to_h, ticket: [9, 1]) }
    assert_raises(TypeError) { H::Status.new(**status.to_h, deadline: 1.0) }
    assert_raises(ArgumentError) { H::Status.new(ticket, 1) }
    assert_raises(ArgumentError) { H::Status.new(**status.to_h, extra: 1) }
    handoff.quiesced(ticket, 1, 12, 1)
    [11, 10].each { |offset| handoff.drained(ticket, 1, offset, 1) }
    handoff.captured(ticket, 1, 77, 1)
    handoff.installed(ticket, 2, 77, 1)
    handoff.fenced(ticket, tok(6), 1)
    granted = handoff.status(9)
    drain = H::Status.new(**granted.to_h, phase: :drain, base: 10, until: 12, checkpoint: nil, token: nil)
    assert_equal :drain, drain.phase
    # A drain whose base reached the quiesce point has moved on to the capture.
    assert_raises(RangeError) { H::Status.new(**drain.to_h, base: 12) }
    assert_equal :checkpoint, H::Status.new(**drain.to_h, base: 12, phase: :checkpoint).phase
    assert_equal granted, H::Status.new(**granted.to_h, token: SiblingToken.new(6))
    assert_instance_of H::FencingToken, H::Status.new(**granted.to_h, token: SiblingToken.new(6)).token
  end

  def test_a_call_that_raises_changed_nothing
    handoff, ticket = begun
    before = snapshot(handoff)
    assert_raises(IOError) { handoff.start(3, 3, 4, 0, Exploding.new, 1) }
    handoff.quiesced(ticket, 1, 10, 1)
    handoff.captured(ticket, 1, 77, 1)
    handoff.installed(ticket, 2, 77, 1)
    waiting = snapshot(handoff)
    refute_equal before, waiting
    assert_raises(IOError) { handoff.fenced(ticket, Exploding.new, 1) }
    assert_raises(IOError) { handoff.abort(ticket, Exploding.new, 1) }
    assert_equal waiting, snapshot(handoff)
  end

  # The classes of an honoured answer are resolved once at load: removing a
  # constant afterwards changes nothing the binding builds.
  def test_answer_classes_are_resolved_at_load
    handoff, ticket = begun
    without(:Recorded) { |recorded| assert_instance_of recorded, handoff.quiesced(ticket, 1, 12, 1) }
    without(:Ticket) { |klass| assert_instance_of klass, handoff.start(3, 3, 4, 0, tok(1), 1) }
  end

  def without(name)
    saved = H.send(:remove_const, name)
    yield saved
  ensure
    H.const_set(name, saved)
  end

  def test_only_a_new_wait_records_the_clock
    handoff, ticket = begun
    assert_equal H::Recorded.new(:drain), handoff.quiesced(ticket, 1, 12, 1)
    assert_equal H::Recorded.new(:drain), handoff.drained(ticket, 1, 11, 50)
    # 50 was not recorded: the drain going on holds whatever the time.
    assert_instance_of H::Ticket, handoff.start(3, 3, 4, 0, tok(1), 2)
    assert_equal H::Recorded.new(:checkpoint), handoff.drained(ticket, 1, 10, 40)
    assert_equal :clock_regressed, handoff.captured(ticket, 1, 77, 39)
  end
end
