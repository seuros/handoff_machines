# frozen_string_literal: true

require 'minitest/autorun'
require 'handoff_machines'

# Every ticket carries the scope of the coordinator that issued it.
class ScopeTest < Minitest::Test
  H = HandoffMachines
  SECOND = 1_000_000_000
  LIMITS = H::Limits.new(5 * SECOND, 2)

  # Partition 9 from replica 1 to replica 2, walked up to the activation order.
  def awaiting_activation(scope)
    handoff = H::Handoff.with_scope(LIMITS, scope)
    ticket = handoff.start(9, 1, 2, 10, 5, 0)
    handoff.quiesced(ticket, 1, 12, 1)
    [11, 10].each { |offset| handoff.drained(ticket, 1, offset, 1) }
    handoff.captured(ticket, 1, 77, 1)
    handoff.installed(ticket, 2, 77, 1)
    handoff.fenced(ticket, 6, 1)
    [handoff, ticket]
  end

  # What every call that takes a ticket answers.
  def answers(handoff, ticket)
    [handoff.quiesced(ticket, 1, 12, 9), handoff.drained(ticket, 1, 10, 9), handoff.committed(ticket, 1, 10, 9),
     handoff.captured(ticket, 1, 77, 9), handoff.installed(ticket, 2, 77, 9), handoff.fenced(ticket, 6, 9),
     handoff.activated(ticket, 2, 6, 9), handoff.abort(ticket, nil, 9), handoff.abort(ticket, 9, 9),
     handoff.resumed(ticket, 1, 9), handoff.discarded(ticket, 2, 9), handoff.outstanding(ticket, 0, 64)]
  end

  def test_new_is_scope_zero_and_issues_the_plain_counter
    handoff = H::Handoff.new(LIMITS)
    assert_equal [0, 0], [handoff.scope, handoff.generation]
    first = handoff.start(1, 1, 2, 0, 1, 0)
    second = handoff.start(2, 3, 4, 0, 1, 0)
    assert_equal [1, 2, 0], [first.generation, second.generation, first.scope]
  end

  def test_with_scope_stamps_every_ticket
    handoff = H::Handoff.with_scope(LIMITS, 9)
    stamp = 9 << 48
    assert_equal [9, stamp, LIMITS], [handoff.scope, handoff.generation, handoff.limits]
    ticket = handoff.start(9, 1, 2, 10, 5, 0)
    assert_equal [stamp | 1, 9], [ticket.generation, ticket.scope]
    assert_equal [ticket, ticket], [handoff.status(9).ticket, handoff.orders(0, 4).first.first.ticket]
    assert_equal 65_535, H::Handoff.with_scope(LIMITS, 65_535).scope
  end

  def test_a_ticket_from_another_coordinator_is_unknown
    near, near_ticket = awaiting_activation(1)
    far, far_ticket = awaiting_activation(2)
    assert_equal [9, 9], [near_ticket.partition, far_ticket.partition]
    refute_equal near_ticket, far_ticket
    [[near, far_ticket], [far, near_ticket]].each do |handoff, foreign|
      before = [handoff.status(9), handoff.orders(0, 4), handoff.generation, handoff.active]
      assert_equal [:unknown], answers(handoff, foreign).uniq
      assert_equal before, [handoff.status(9), handoff.orders(0, 4), handoff.generation, handoff.active]
    end
    assert_equal H::Duplicate.new(:activate), near.quiesced(near_ticket, 1, 12, 9)
    assert_equal H::Recorded.new(:complete), far.activated(far_ticket, 2, 6, 9)
    assert_equal [:stale], answers(far, far_ticket).uniq
    assert_equal [:unknown], answers(near, far_ticket).uniq
  end

  def test_two_coordinators_left_in_one_scope_still_alias
    one, = awaiting_activation(5)
    _, theirs = awaiting_activation(5)
    assert_equal H::Duplicate.new(:activate), one.quiesced(theirs, 1, 12, 9)
    assert_equal H::Recorded.new(:complete), one.activated(theirs, 2, 6, 9)
  end

  def test_restored_round_trips_fresh_and_used_coordinators
    [0, 9, 65_535].each do |scope|
      fresh = H::Handoff.with_scope(LIMITS, scope)
      again = H::Handoff.restored(LIMITS, scope, fresh.generation)
      assert_equal [scope, scope << 48, LIMITS], [again.scope, again.generation, again.limits]
      old = fresh.start(9, 1, 2, 10, 5, 0)
      again = H::Handoff.restored(LIMITS, scope, fresh.generation)
      assert_equal [scope, old.generation], [again.scope, again.generation]
      assert_equal :stale, again.quiesced(old, 1, 12, 0)
      assert_equal old.generation + 1, again.start(9, 1, 2, 10, 5, 0).generation
      assert_equal :unknown, again.quiesced(H::Ticket.new(9, old.generation + 2), 1, 12, 0)
    end
  end

  def test_restored_refuses_a_saved_value_of_another_scope
    handoff = H::Handoff.with_scope(LIMITS, 9)
    handoff.start(9, 1, 2, 10, 5, 0)
    saved = handoff.generation
    # A missing row read as 0, a value cut to 32 bits, and another scope's.
    [[9, 0], [65_535, 0], [9, saved & 0xFFFF_FFFF], [0, saved], [8, saved], [0, 9 << 48]].each do |scope, bad|
      error = assert_raises(RangeError) { H::Handoff.restored(LIMITS, scope, bad) }
      assert_equal 'last_generation is not a generation of this scope', error.message
    end
    [-1, 65_536].each { |bad| assert_raises(RangeError) { H::Handoff.restored(LIMITS, bad, 0) } }
    assert_raises(ArgumentError) { H::Handoff.restored(LIMITS, saved) }
  end

  def test_scope_is_a_strict_integer
    [1.0, nil, '1', 1r, Object.new].each { |bad| assert_raises(TypeError) { H::Handoff.with_scope(LIMITS, bad) } }
    [-1, 65_536, 2**64].each { |bad| assert_raises(RangeError) { H::Handoff.with_scope(LIMITS, bad) } }
    assert_raises(TypeError) { H::Handoff.with_scope([5, 2], 1) }
    assert_raises(ArgumentError) { H::Handoff.with_scope(LIMITS) }
    assert_raises(NoMethodError) { H::Handoff.native_with_scope(1, 1, 0) }
    assert_raises(NoMethodError) { H::Handoff.native_scope_of(1) }
    assert_raises(NoMethodError) { H::Handoff.fields(LIMITS) }
  end
end
