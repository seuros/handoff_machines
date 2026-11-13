# frozen_string_literal: true

require 'minitest/autorun'
require 'handoff_machines'

# Fencing tokens are values that carry their authority's scope.
class TokenTest < Minitest::Test
  H = HandoffMachines
  SECOND = 1_000_000_000
  LIMITS = H::Limits.new(5 * SECOND, 2)
  HOME = 3

  # What the `lease_machines` gem's token looks like to this one: another
  # class with the same reader.
  SiblingToken = Data.define(:raw)

  def tok(scope, sequence) = H::FencingToken.from_raw((scope << 48) | sequence)
  def rec(phase) = H::Recorded.new(phase)
  def snapshot(handoff) = [handoff.status(9), handoff.orders(0, 32), handoff.generation, handoff.active]

  # Tokens of other scopes: raw values below, above and far above HOME's.
  def foreign = [tok(0, 99), tok(HOME - 1, 6), tok(HOME + 1, 1), tok(65_535, 6)]

  # Partition 9 from replica 1 to replica 2 under source token 5 of HOME,
  # waiting for the authority's token.
  def awaiting_fence
    handoff = H::Handoff.new(LIMITS)
    ticket = handoff.start(9, 1, 2, 10, tok(HOME, 5), 0)
    handoff.quiesced(ticket, 1, 10, 1)
    handoff.captured(ticket, 1, 77, 1)
    handoff.installed(ticket, 2, 77, 1)
    [handoff, ticket]
  end

  def assert_refused(refusal, handoff)
    assert_includes H::REFUSALS, refusal
    before = snapshot(handoff)
    assert_equal refusal, yield
    assert_equal before, snapshot(handoff), "#{refusal} changed something"
  end

  def test_a_token_is_a_scoped_value_without_an_order_across_scopes
    one = tok(HOME, 1)
    assert_equal [HOME, (HOME << 48) | 1], [one.scope, one.raw]
    assert_equal [-1, 0, 1], [one.order(tok(HOME, 2)), one.order(one), tok(HOME, 2).order(one)]
    assert_nil one.order(tok(HOME + 1, 1))
    refute tok(HOME + 1, 9).newer_than?(one)
    assert tok(HOME, 2).newer_than?(SiblingToken.new(one.raw))
    refute_respond_to one, :<
    assert_raises(RangeError) { H::FencingToken.from_raw(0) }
    assert_raises(RangeError) { H::FencingToken.from_raw(HOME << 48) }
    assert_raises(RangeError) { H::FencingToken.new(2**64) }
    assert_raises(TypeError) { H::FencingToken.new(1.0) }
    assert_raises(TypeError) { one.order(1) }
  end

  def test_orders_and_status_answer_token_values
    handoff, ticket = awaiting_fence
    assert_equal [H::Order.new(ticket, :authority, H::Fence.new(tok(HOME, 5)))], handoff.orders(0, 32).first
    assert_nil handoff.status(9).token
    assert_equal rec(:activate), handoff.fenced(ticket, tok(HOME, 6), 2)
    assert_equal tok(HOME, 6), handoff.status(9).token
    assert_equal H::Activate.new(tok(HOME, 6)), handoff.orders(0, 32).first.first.action
    assert_equal rec(:rollback), handoff.abort(ticket, tok(HOME, 7), 3)
    assert_equal H::Resume.new(tok(HOME, 7)), handoff.orders(0, 32).first.first.action
  end

  def test_a_token_of_another_scope_is_unknown_whatever_its_number
    handoff, ticket = awaiting_fence
    foreign.each do |token|
      assert_refused(:unknown, handoff) { handoff.fenced(ticket, token, 2) }
      assert_refused(:unknown, handoff) { handoff.abort(ticket, token, 2) }
    end
    [1, 5].each { |old| assert_refused(:stale_token, handoff) { handoff.fenced(ticket, tok(HOME, old), 2) } }
    assert_equal rec(:activate), handoff.fenced(ticket, tok(HOME, 6), 2)
    foreign.each { |token| assert_refused(:unknown, handoff) { handoff.abort(ticket, token, 3) } }
    assert_refused(:stale_token, handoff) { handoff.abort(ticket, tok(HOME, 6), 3) }
    assert_refused(:mismatch, handoff) { handoff.fenced(ticket, tok(HOME, 7), 3) }
    assert_refused(:mismatch, handoff) { handoff.activated(ticket, 2, tok(HOME, 7), 3) }
  end

  # Scope is read first: a foreign token is :unknown, never :mismatch against
  # a recorded one and never :out_of_phase.
  def test_a_foreign_token_is_unknown_before_the_record_and_the_phase
    handoff, ticket = awaiting_fence
    assert_equal rec(:activate), handoff.fenced(ticket, tok(HOME, 6), 2)
    twin = tok(HOME + 1, 6)
    assert_refused(:unknown, handoff) { handoff.fenced(ticket, twin, 3) }
    assert_refused(:unknown, handoff) { handoff.activated(ticket, 2, twin, 3) }
    assert_equal rec(:rollback), handoff.abort(ticket, tok(HOME, 7), 3)
    foreign.each do |token|
      assert_refused(:unknown, handoff) { handoff.abort(ticket, token, 4) }
      assert_refused(:unknown, handoff) { handoff.fenced(ticket, token, 4) }
      assert_refused(:unknown, handoff) { handoff.activated(ticket, 2, token, 4) }
    end
    assert_refused(:mismatch, handoff) { handoff.abort(ticket, tok(HOME, 8), 4) }
    assert_equal H::Duplicate.new(:rollback), handoff.abort(ticket, tok(HOME, 7), 4)

    early = H::Handoff.new(LIMITS)
    ticket = early.start(9, 1, 2, 10, tok(HOME, 5), 0)
    foreign.each do |token|
      assert_refused(:unknown, early) { early.fenced(ticket, token, 1) }
      [1, 2, 3].each { |who| assert_refused(:unknown, early) { early.activated(ticket, who, token, 1) } }
    end
    assert_refused(:out_of_phase, early) { early.fenced(ticket, tok(HOME, 6), 1) }
    assert_refused(:out_of_phase, early) { early.activated(ticket, 2, tok(HOME, 6), 1) }
    # The clock and the ticket still come before the token.
    assert_refused(:unknown, early) { early.fenced(H::Ticket.new(9, ticket.generation + 1), foreign.first, 1) }
    assert_equal rec(:rollback), early.abort(ticket, nil, 5)
    assert_refused(:clock_regressed, early) { early.fenced(ticket, foreign.first, 4) }
  end

  def test_a_transfer_behind_a_rescoped_authority_can_still_be_given_up
    handoff, ticket = awaiting_fence
    assert_refused(:unknown, handoff) { handoff.fenced(ticket, tok(HOME + 1, 1), 2) }
    assert_equal rec(:rollback), handoff.abort(ticket, nil, 2)
  end

  def test_the_lease_gems_token_fits_by_its_reader
    handoff = H::Handoff.new(LIMITS)
    held = SiblingToken.new((HOME << 48) | 5)
    granted = SiblingToken.new((HOME << 48) | 6)
    ticket = handoff.start(9, 1, 2, 10, held, 0)
    handoff.quiesced(ticket, 1, 10, 1)
    handoff.captured(ticket, 1, 77, 1)
    handoff.installed(ticket, 2, 77, 1)
    assert_equal held.raw, handoff.orders(0, 32).first.first.action.above.raw
    assert_equal rec(:activate), handoff.fenced(ticket, granted, 2)
    assert_equal rec(:complete), handoff.activated(ticket, 2, granted, 3)
  end

  def test_a_bare_integer_is_not_a_token
    handoff, ticket = awaiting_fence
    before = snapshot(handoff)
    assert_raises(TypeError) { handoff.start(1, 3, 4, 0, 1, 0) }
    assert_raises(TypeError) { handoff.fenced(ticket, 6, 2) }
    assert_raises(TypeError) { handoff.activated(ticket, 2, 6, 2) }
    assert_raises(TypeError) { handoff.abort(ticket, 7, 2) }
    assert_raises(TypeError) { handoff.fenced(ticket, SiblingToken.new('6'), 2) }
    assert_raises(RangeError) { handoff.fenced(ticket, SiblingToken.new(0), 2) }
    assert_raises(RangeError) { handoff.abort(ticket, SiblingToken.new(2**64), 2) }
    assert_equal before, snapshot(handoff)
  end
end
