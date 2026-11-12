# frozen_string_literal: true
require 'minitest/autorun'
require 'handoff_machines'

class CoordinatorTest < Minitest::Test
  H = HandoffMachines
  SECOND = 1_000_000_000

  def coordinator(per_replica: 2, **)
    H::Coordinator.new(phase_budget_ns: 5 * SECOND, per_replica:, **)
  end

  # Partition 9 from replica 1 to replica 2, offsets below 10 committed, source token 5.
  def begun(handoff = coordinator, partition: 9)
    ticket = handoff.start(partition, source: 1, target: 2, base: 10, token: 5, now_ns: 0)
    [handoff, ticket]
  end

  def advanced(steps)
    handoff, ticket = begun
    [
      -> { handoff.quiesced(ticket, 1, 12, 1) },
      -> { [11, 10].each { |offset| handoff.drained(ticket, 1, offset, 1) } },
      -> { handoff.captured(ticket, 1, 77, 1) },
      -> { handoff.installed(ticket, 2, 77, 1) },
      -> { handoff.fenced(ticket, 6, 1) }
    ].first(steps).each(&:call)
    [handoff, ticket]
  end

  def test_constants_and_data
    assert_equal 16, H::CAPACITY
    assert_equal 256, H::WINDOW
    assert_equal %i[quiesce drain checkpoint activate rollback complete aborted], H::PHASES
    assert_equal 18, H::REFUSALS.size
    assert H::PHASES.frozen? && H::REFUSALS.frozen?
    assert_empty H::PHASES & H::REFUSALS
    [H::Ticket, H::Order, H::Status].each { |type| assert_operator type, :<, Data }
    assert_equal H::Ticket.new(1, 2), H::Ticket.new(partition: 1, generation: 2)
  end

  def test_happy_path_and_orders
    handoff, ticket = begun
    assert_equal H::Ticket.new(9, 1), ticket
    assert_equal [1, 1], [handoff.active, handoff.generation]
    assert_equal [[H::Order.new(ticket, 1, :quiesce, nil)], 32], handoff.orders
    assert_equal :drain, handoff.quiesced(ticket, 1, 12, 1)
    assert_equal [H::Order.new(ticket, 1, :drain, 12)], handoff.orders.first
    assert_equal :drain, handoff.drained(ticket, 1, 11, 1)
    assert_equal [10], handoff.outstanding(ticket)
    assert_equal :checkpoint, handoff.drained(ticket, 1, 10, 2)
    assert_equal :out_of_phase, handoff.outstanding(ticket)
    assert_equal [H::Order.new(ticket, 1, :capture, 12)], handoff.orders.first
    assert_equal :checkpoint, handoff.captured(ticket, 1, 77, 2)
    assert_equal [H::Order.new(ticket, 2, :install, 77)], handoff.orders.first
    assert_equal :activate, handoff.installed(ticket, 2, 77, 3)
    assert_equal [H::Order.new(ticket, :authority, :fence, 5)], handoff.orders.first
    assert_equal :activate, handoff.fenced(ticket, 6, 3)
    assert_equal [H::Order.new(ticket, 2, :activate, 6)], handoff.orders.first
    expected = H::Status.new(ticket, 1, 2, :activate, 10, 12, 77, 6, 3 + 5 * SECOND)
    assert_equal expected, handoff.status(9)
    assert_equal :complete, handoff.activated(ticket, 2, 6, 4)
    assert_equal [0, nil, []], [handoff.active, handoff.status(9), handoff.orders.first]
    assert_equal :stale, handoff.activated(ticket, 2, 6, 4)
  end

  def test_abort_from_each_phase
    { 0 => :quiesce, 1 => :drain, 2 => :checkpoint, 3 => :checkpoint, 4 => :activate }.each do |steps, phase|
      handoff, ticket = advanced(steps)
      assert_equal phase, handoff.status(9).phase
      assert_equal :rollback, handoff.abort(ticket, 2)
      assert_equal :out_of_phase, handoff.abort(ticket, 2)
      owed = [H::Order.new(ticket, 1, :resume, nil)]
      owed << H::Order.new(ticket, 2, :discard, nil) if steps >= 3
      assert_equal owed, handoff.orders.first
      assert_equal :wrong_replica, handoff.resumed(ticket, 2, 2)
      if steps >= 3
        assert_equal :rollback, handoff.discarded(ticket, 2, 2)
        assert_equal :duplicate, handoff.discarded(ticket, 2, 2)
      else
        assert_equal :duplicate, handoff.discarded(ticket, 2, 2)
      end
      assert_equal :aborted, handoff.resumed(ticket, 1, 3)
      assert_equal 0, handoff.active
    end
  end

  def test_abort_after_activation_needs_a_newer_token
    handoff, ticket = advanced(5)
    assert_equal :fence_required, handoff.abort(ticket, 2)
    assert_equal :stale_token, handoff.abort(ticket, 2, token: 6)
    assert_equal :rollback, handoff.abort(ticket, 2, token: 7)
    assert_equal [H::Order.new(ticket, 1, :resume, 7), H::Order.new(ticket, 2, :discard, nil)], handoff.orders.first
    assert_equal :rollback, handoff.resumed(ticket, 1, 2)
    assert_equal :aborted, handoff.discarded(ticket, 2, 2)
  end

  def test_stale_generations_are_refused
    handoff, old = begun
    handoff.abort(old, 0)
    handoff.resumed(old, 1, 0)
    _, fresh = begun(handoff)
    assert_equal 2, fresh.generation
    forged = H::Ticket.new(9, 3)
    { old => :stale, forged => :unknown }.each do |ticket, refusal|
      assert_equal refusal, handoff.quiesced(ticket, 1, 12, 0)
      assert_equal refusal, handoff.drained(ticket, 1, 10, 0)
      assert_equal refusal, handoff.committed(ticket, 1, 11, 0)
      assert_equal refusal, handoff.captured(ticket, 1, 7, 0)
      assert_equal refusal, handoff.installed(ticket, 2, 7, 0)
      assert_equal refusal, handoff.fenced(ticket, 6, 0)
      assert_equal refusal, handoff.activated(ticket, 2, 6, 0)
      assert_equal refusal, handoff.abort(ticket, 0)
      assert_equal refusal, handoff.resumed(ticket, 1, 0)
      assert_equal refusal, handoff.discarded(ticket, 2, 0)
      assert_equal refusal, handoff.outstanding(ticket)
    end
    assert_equal :quiesce, handoff.status(9).phase
    assert_equal :drain, handoff.quiesced(fresh, 1, 12, 0)
  end

  def test_evidence_must_match
    handoff, ticket = begun
    assert_equal :wrong_replica, handoff.quiesced(ticket, 2, 12, 0)
    assert_equal :behind_base, handoff.quiesced(ticket, 1, 9, 0)
    assert_equal :drain, handoff.quiesced(ticket, 1, 300, 0)
    assert_equal :beyond_drain, handoff.drained(ticket, 1, 300, 0)
    assert_equal :out_of_window, handoff.drained(ticket, 1, 10 + H::WINDOW, 0)
    assert_equal :unacked, handoff.committed(ticket, 1, 11, 0)
    assert_equal :drain, handoff.drained(ticket, 1, 10, 0)
    assert_equal :duplicate, handoff.drained(ticket, 1, 10, 0)
    assert_equal :drain, handoff.committed(ticket, 1, 11, 0)
    assert_equal :duplicate, handoff.committed(ticket, 1, 11, 0)
    assert_equal 11, handoff.status(9).base
    assert_equal [11, 12, 13], handoff.outstanding(ticket, limit: 3)
    assert_equal [298, 299], handoff.outstanding(ticket, 298)
    handoff, ticket = advanced(3)
    assert_equal :mismatch, handoff.installed(ticket, 2, 78, 1)
    handoff, ticket = advanced(4)
    assert_equal :stale_token, handoff.fenced(ticket, 5, 1)
    handoff, ticket = advanced(5)
    assert_equal :mismatch, handoff.activated(ticket, 2, 7, 1)
  end

  def test_bounded_concurrent_transfers
    handoff = coordinator(per_replica: 16)
    tickets = H::CAPACITY.times.map { |i| handoff.start(i, source: 1, target: 2, base: 0, token: 1, now_ns: 0) }
    assert_equal (1..16).to_a, tickets.map(&:generation)
    assert_equal :full, handoff.start(99, source: 1, target: 2, base: 0, token: 1, now_ns: 0)
    assert_equal :busy, handoff.start(0, source: 1, target: 2, base: 0, token: 1, now_ns: 0)
    assert_equal :same_replica, handoff.start(99, source: 1, target: 1, base: 0, token: 1, now_ns: 0)
    assert_equal :rollback, handoff.abort(tickets.first, 0)
    assert_equal :aborted, handoff.resumed(tickets.first, 1, 0)
    assert_equal 17, handoff.start(99, source: 1, target: 2, base: 0, token: 1, now_ns: 0).generation
    limited, = begun
    assert_instance_of H::Ticket, limited.start(1, source: 2, target: 3, base: 0, token: 1, now_ns: 0)
    assert_equal :replica_busy, limited.start(2, source: 4, target: 2, base: 0, token: 1, now_ns: 0)
    exhausted = coordinator(last_generation: 2**64 - 1)
    assert_equal :exhausted, exhausted.start(1, source: 1, target: 2, base: 0, token: 1, now_ns: 0)
    assert_equal 2**64 - 1, exhausted.generation
  end

  def test_orders_and_overdue_resume_in_bounded_chunks
    handoff = coordinator(per_replica: 16)
    tickets = 5.times.map { |i| handoff.start(i, source: 1, target: 2, base: 0, token: 1, now_ns: i * SECOND) }
    all, = handoff.orders
    assert_equal tickets, all.map(&:ticket)
    seen = []
    position = 0
    loop do
      chunk, position = handoff.orders(position, limit: 2)
      break if chunk.empty?

      assert_operator chunk.size, :<=, 2
      seen.concat(chunk)
    end
    assert_equal all, seen
    assert_equal [[], 16], handoff.overdue(4 * SECOND)
    assert_equal [tickets.first(3), 16], handoff.overdue(7 * SECOND)
    first, position = handoff.overdue(9 * SECOND, limit: 2)
    assert_equal [tickets.first(2), 2], [first, position]
    assert_equal tickets[2..], handoff.overdue(9 * SECOND, position).first
  end

  def test_time_only_moves_forward
    handoff, ticket = begun
    assert_equal :drain, handoff.quiesced(ticket, 1, 12, 3)
    assert_equal :clock_regressed, handoff.drained(ticket, 1, 10, 2)
    assert_equal :clock_regressed, handoff.abort(ticket, 2)
    assert_equal :clock_regressed, handoff.start(1, source: 3, target: 4, base: 0, token: 1, now_ns: 2)
    assert_equal :drain, handoff.drained(ticket, 1, 10, 3)
  end

  def test_validation
    handoff, ticket = begun
    [1.0, nil, '1', Float::NAN].each do |bad|
      assert_raises(TypeError) { H::Coordinator.new(phase_budget_ns: bad, per_replica: 1) }
      assert_raises(TypeError) { handoff.start(bad, source: 1, target: 2, base: 0, token: 1, now_ns: 0) }
      assert_raises(TypeError) { handoff.start(1, source: 1, target: 2, base: 0, token: bad, now_ns: 0) }
      assert_raises(TypeError) { handoff.quiesced(ticket, 1, 12, bad) }
      assert_raises(TypeError) { handoff.quiesced(bad, 1, 12, 0) }
      assert_raises(TypeError) { handoff.status(bad) }
      assert_raises(TypeError) { handoff.overdue(bad) }
    end
    assert_raises(TypeError) { handoff.abort(ticket, 0, token: '7') }
    assert_raises(TypeError) { handoff.fenced([9, 1], 6, 0) }
    [-1, 2**64].each do |bad|
      assert_raises(RangeError) { handoff.quiesced(ticket, bad, 12, 0) }
      assert_raises(RangeError) { handoff.orders(bad) }
      assert_raises(RangeError) { H::Coordinator.new(phase_budget_ns: 1, per_replica: bad) }
    end
    assert_raises(RangeError) { handoff.fenced(ticket, 0, 0) }
    assert_raises(RangeError) { handoff.start(1, source: 3, target: 4, base: 0, token: 0, now_ns: 0) }
    assert_raises(ArgumentError) { H::Coordinator.new(per_replica: 1) }
    assert_raises(TypeError) { H::Coordinator.allocate }
    assert_raises(NoMethodError) { handoff.native_abort(9, 1, nil, 0) }
    assert_equal :quiesce, handoff.status(9).phase
  end

  def test_u64_limits_and_freeze
    top = 2**64 - 1
    handoff = coordinator(last_generation: top - 1)
    ticket = handoff.start(top, source: top, target: 0, base: top - 1, token: top, now_ns: top)
    assert_equal H::Ticket.new(top, top), ticket
    assert_equal top, handoff.status(top).deadline_ns
    assert_equal :drain, handoff.quiesced(ticket, top, top, top)
    assert_equal [top - 1], handoff.outstanding(ticket)
    # The real deadline is past what an Integer of nanoseconds can say, so it is reported saturated.
    assert_equal [[], 16], handoff.overdue(top)
    handoff.freeze
    assert_raises(FrozenError) { handoff.drained(ticket, top, top - 1, top) }
    assert_raises(FrozenError) { handoff.abort(ticket, top) }
    assert_raises(FrozenError) { handoff.start(1, source: 1, target: 2, base: 0, token: 1, now_ns: top) }
    assert_equal :drain, handoff.status(top).phase
    assert_equal 1, handoff.orders.first.size
  end
end
