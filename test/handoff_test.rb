# frozen_string_literal: true
require 'minitest/autorun'
require 'handoff_machines'

class HandoffTest < Minitest::Test
  H = HandoffMachines
  SECOND = 1_000_000_000
  LIMITS = H::Limits.new(5 * SECOND, 2)

  def tok(raw) = H::FencingToken.from_raw(raw)
  def rec(phase) = H::Recorded.new(phase)
  def again(phase) = H::Duplicate.new(phase)
  def replica(id) = H::Replica.new(id)
  def orders(handoff) = handoff.orders(0, 2 * H::CAPACITY).first
  def missing(handoff, ticket) = handoff.outstanding(ticket, 0, H::WINDOW)

  # Partition 9 from replica 1 to replica 2, offsets below 10 committed, source token 5.
  def begun(handoff = H::Handoff.new(LIMITS), partition: 9)
    [handoff, handoff.start(partition, 1, 2, 10, tok(5), 0)]
  end

  def advanced(steps)
    handoff, ticket = begun
    [
      -> { handoff.quiesced(ticket, 1, 12, 1) },
      -> { [11, 10].each { |offset| handoff.drained(ticket, 1, offset, 1) } },
      -> { handoff.captured(ticket, 1, 77, 1) },
      -> { handoff.installed(ticket, 2, 77, 1) },
      -> { handoff.fenced(ticket, tok(6), 1) }
    ].first(steps).each(&:call)
    [handoff, ticket]
  end

  def test_constants_and_data
    assert_equal 16, H::CAPACITY
    assert_equal 256, H::WINDOW
    assert_equal %i[quiesce drain checkpoint activate rollback complete aborted], H::PHASES
    assert_equal 17, H::REFUSALS.size
    assert H::PHASES.frozen? && H::REFUSALS.frozen?
    assert_empty H::PHASES & H::REFUSALS
    assert_equal %i[phase_budget per_replica], H::Limits.members
    assert_equal %i[partition generation], H::Ticket.members
    assert_equal %i[ticket to action], H::Order.members
    assert_equal %i[ticket source target phase base until checkpoint token deadline], H::Status.members
    fields = { Recorded: :phase, Duplicate: :phase, Replica: :id, Drain: :until, Capture: :at,
               Install: :checkpoint, Fence: :above, Activate: :token, Resume: :token }
    fields.each do |name, field|
      assert_operator H.const_get(name), :<, Data
      assert_equal [field], H.const_get(name).members
    end
    assert_equal H::Ticket.new(1, 2), H::Ticket.new(partition: 1, generation: 2)
    assert_equal [false, true, :drain], [rec(:drain).duplicate?, again(:drain).duplicate?, again(:drain).phase]
  end

  def test_construction_mirrors_the_core
    handoff = H::Handoff.new(LIMITS)
    assert_equal [LIMITS, 0, 0], [handoff.limits, handoff.generation, handoff.active]
    restored = H::Handoff.restored(LIMITS, 0, 40)
    assert_equal [LIMITS, 40], [restored.limits, restored.generation]
    assert_equal H::Ticket.new(9, 41), restored.start(9, 1, 2, 10, tok(5), 0)
  end

  def test_happy_path_and_orders
    handoff, ticket = begun
    assert_equal H::Ticket.new(9, 1), ticket
    assert_equal [1, 1], [handoff.active, handoff.generation]
    assert_equal [[H::Order.new(ticket, replica(1), :quiesce)], 32], handoff.orders(0, 32)
    assert_equal rec(:drain), handoff.quiesced(ticket, 1, 12, 1)
    assert_equal [H::Order.new(ticket, replica(1), H::Drain.new(12))], orders(handoff)
    assert_equal rec(:drain), handoff.drained(ticket, 1, 11, 1)
    assert_equal [10], missing(handoff, ticket)
    assert_equal rec(:checkpoint), handoff.drained(ticket, 1, 10, 2)
    assert_equal :out_of_phase, missing(handoff, ticket)
    assert_equal [H::Order.new(ticket, replica(1), H::Capture.new(12))], orders(handoff)
    assert_equal rec(:checkpoint), handoff.captured(ticket, 1, 77, 2)
    assert_equal [H::Order.new(ticket, replica(2), H::Install.new(77))], orders(handoff)
    assert_equal rec(:activate), handoff.installed(ticket, 2, 77, 3)
    assert_equal [H::Order.new(ticket, :authority, H::Fence.new(tok(5)))], orders(handoff)
    assert_equal rec(:activate), handoff.fenced(ticket, tok(6), 3)
    assert_equal [H::Order.new(ticket, replica(2), H::Activate.new(tok(6)))], orders(handoff)
    expected = H::Status.new(ticket, 1, 2, :activate, 10, 12, 77, tok(6), 3 + (5 * SECOND))
    assert_equal expected, handoff.status(9)
    assert_equal rec(:complete), handoff.activated(ticket, 2, tok(6), 4)
    assert_equal [0, nil, []], [handoff.active, handoff.status(9), orders(handoff)]
    assert_equal :stale, handoff.activated(ticket, 2, tok(6), 4)
  end

  def test_abort_from_each_phase
    { 0 => :quiesce, 1 => :drain, 2 => :checkpoint, 3 => :checkpoint, 4 => :activate }.each do |steps, phase|
      handoff, ticket = advanced(steps)
      assert_equal phase, handoff.status(9).phase
      assert_equal rec(:rollback), handoff.abort(ticket, nil, 2)
      owed = [H::Order.new(ticket, replica(1), H::Resume.new(nil))]
      owed << H::Order.new(ticket, replica(2), :discard) if steps >= 3
      assert_equal owed, orders(handoff)
      assert_equal :wrong_replica, handoff.resumed(ticket, 2, 2)
      assert_equal rec(:rollback), handoff.discarded(ticket, 2, 2) if steps >= 3
      # Already recorded, or never owed: an honoured repeat that changes nothing, not even the clock.
      before = [handoff.status(9), orders(handoff)]
      assert_equal again(:rollback), handoff.discarded(ticket, 2, 99)
      assert_equal before, [handoff.status(9), orders(handoff)]
      assert_equal rec(:aborted), handoff.resumed(ticket, 1, 3)
      assert_equal 0, handoff.active
    end
  end

  def test_abort_after_activation_needs_a_newer_token
    handoff, ticket = advanced(5)
    assert_equal :fence_required, handoff.abort(ticket, nil, 2)
    assert_equal :stale_token, handoff.abort(ticket, tok(6), 2)
    assert_equal rec(:rollback), handoff.abort(ticket, tok(7), 2)
    owed = [H::Order.new(ticket, replica(1), H::Resume.new(tok(7))), H::Order.new(ticket, replica(2), :discard)]
    assert_equal owed, orders(handoff)
    assert_equal rec(:rollback), handoff.resumed(ticket, 1, 2)
    assert_equal again(:rollback), handoff.resumed(ticket, 1, 2)
    assert_equal rec(:aborted), handoff.discarded(ticket, 2, 2)
  end

  def view(handoff)
    [handoff.status(9), handoff.orders(0, 32), handoff.overdue(6 * SECOND, 0, H::CAPACITY)]
  end

  # [call, answer] pairs that must leave the coordinator exactly as it was.
  def assert_unchanged(handoff, calls)
    before = view(handoff)
    calls.each do |call, answer|
      assert_equal answer, call.call, "line #{call.source_location.last}"
      assert_equal before, view(handoff)
    end
  end

  def forward(handoff, ticket, late)
    [
      -> { handoff.quiesced(ticket, 1, 12, late) },
      -> { handoff.drained(ticket, 1, 10, late) },
      -> { handoff.committed(ticket, 1, 10, late) },
      -> { handoff.captured(ticket, 1, 77, late) },
      -> { handoff.installed(ticket, 2, 77, late) },
      -> { handoff.fenced(ticket, tok(6), late) }
    ]
  end

  def test_recorded_acknowledgements_repeat_until_the_transfer_ends
    late = 4 * SECOND
    handoff, ticket = advanced(5)
    assert_unchanged(handoff, forward(handoff, ticket, late).map { |call| [call, again(:activate)] })
    assert_equal 1 + (5 * SECOND), handoff.status(9).deadline
    assert_equal rec(:rollback), handoff.abort(ticket, tok(7), 2)
    assert_unchanged(handoff, forward(handoff, ticket, late).map { |call| [call, again(:rollback)] })
    assert_unchanged(
      handoff,
      [
        [-> { handoff.abort(ticket, tok(7), late) }, again(:rollback)],
        [-> { handoff.abort(ticket, tok(8), late) }, :mismatch],
        [-> { handoff.abort(ticket, nil, late) }, :mismatch],
        [-> { handoff.activated(ticket, 2, tok(6), late) }, :out_of_phase],
        [-> { handoff.quiesced(ticket, 1, 12, 0) }, :clock_regressed]
      ]
    )
    assert_equal rec(:rollback), handoff.resumed(ticket, 1, 3)
    assert_equal rec(:aborted), handoff.discarded(ticket, 2, 3)
    (forward(handoff, ticket, late) << -> { handoff.abort(ticket, tok(7), late) }).each do |call|
      assert_equal :stale, call.call
    end
  end

  def test_only_what_was_recorded_repeats
    # Aborted at every step: what came before repeats, what never came is out of phase.
    needed = [1, 2, 1, 3, 4, 5]
    (0..4).each do |steps|
      handoff, ticket = advanced(steps)
      assert_equal rec(:rollback), handoff.abort(ticket, nil, 2)
      answers = needed.map { |step| steps >= step ? again(:rollback) : :out_of_phase }
      assert_unchanged(handoff, forward(handoff, ticket, 4 * SECOND).zip(answers))
      assert_unchanged(handoff, [[-> { handoff.abort(ticket, nil, 9) }, again(:rollback)]])
    end
    handoff, ticket = begun
    early = forward(handoff, ticket, 0).drop(1) << -> { handoff.activated(ticket, 2, tok(6), 0) }
    early.push(-> { handoff.resumed(ticket, 1, 0) }, -> { handoff.discarded(ticket, 2, 0) })
    assert_unchanged(handoff, early.map { |call| [call, :out_of_phase] })
  end

  def test_a_replay_that_differs_is_refused
    handoff, ticket = advanced(5)
    assert_unchanged(
      handoff,
      [
        [-> { handoff.quiesced(ticket, 1, 13, 9) }, :mismatch],
        [-> { handoff.quiesced(ticket, 2, 12, 9) }, :wrong_replica],
        [-> { handoff.captured(ticket, 1, 78, 9) }, :mismatch],
        [-> { handoff.captured(ticket, 2, 77, 9) }, :wrong_replica],
        [-> { handoff.installed(ticket, 2, 78, 9) }, :mismatch],
        [-> { handoff.installed(ticket, 1, 77, 9) }, :wrong_replica],
        [-> { handoff.fenced(ticket, tok(7), 9) }, :mismatch],
        [-> { handoff.fenced(ticket, tok(5), 9) }, :mismatch],
        [-> { handoff.drained(ticket, 1, 12, 9) }, :beyond_drain],
        [-> { handoff.drained(ticket, 2, 10, 9) }, :wrong_replica],
        [-> { handoff.committed(ticket, 1, 11, 9) }, :out_of_phase],
        [-> { handoff.committed(ticket, 2, 10, 9) }, :wrong_replica]
      ]
    )
  end

  def test_stale_generations_are_refused
    handoff, old = begun
    handoff.abort(old, nil, 0)
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
      assert_equal refusal, handoff.fenced(ticket, tok(6), 0)
      assert_equal refusal, handoff.activated(ticket, 2, tok(6), 0)
      assert_equal refusal, handoff.abort(ticket, nil, 0)
      assert_equal refusal, handoff.resumed(ticket, 1, 0)
      assert_equal refusal, handoff.discarded(ticket, 2, 0)
      assert_equal refusal, missing(handoff, ticket)
    end
    assert_equal :quiesce, handoff.status(9).phase
    assert_equal rec(:drain), handoff.quiesced(fresh, 1, 12, 0)
  end

  def test_evidence_must_match
    handoff, ticket = begun
    assert_equal :wrong_replica, handoff.quiesced(ticket, 2, 12, 0)
    assert_equal :behind_base, handoff.quiesced(ticket, 1, 9, 0)
    assert_equal rec(:drain), handoff.quiesced(ticket, 1, 300, 0)
    assert_equal :beyond_drain, handoff.drained(ticket, 1, 300, 0)
    assert_equal :out_of_window, handoff.drained(ticket, 1, 10 + H::WINDOW, 0)
    assert_equal :unacked, handoff.committed(ticket, 1, 11, 0)
    assert_equal rec(:drain), handoff.drained(ticket, 1, 10, 0)
    before = [handoff.status(9), handoff.outstanding(ticket, 0, 3)]
    assert_equal again(:drain), handoff.drained(ticket, 1, 10, 50)
    assert_equal before, [handoff.status(9), handoff.outstanding(ticket, 0, 3)]
    assert_equal rec(:drain), handoff.committed(ticket, 1, 11, 0)
    assert_equal again(:drain), handoff.committed(ticket, 1, 11, 50)
    assert_equal again(:drain), handoff.drained(ticket, 1, 10, 50)
    assert_equal 11, handoff.status(9).base
    assert_equal [11, 12, 13], handoff.outstanding(ticket, 0, 3)
    assert_equal [298, 299], handoff.outstanding(ticket, 298, H::WINDOW)
    handoff, ticket = advanced(3)
    assert_equal :mismatch, handoff.installed(ticket, 2, 78, 1)
    handoff, ticket = advanced(4)
    assert_equal :stale_token, handoff.fenced(ticket, tok(5), 1)
    handoff, ticket = advanced(5)
    assert_equal :mismatch, handoff.activated(ticket, 2, tok(7), 1)
  end

  def test_bounded_concurrent_transfers
    handoff = H::Handoff.new(H::Limits.new(5 * SECOND, 16))
    tickets = H::CAPACITY.times.map { |i| handoff.start(i, 1, 2, 0, tok(1), 0) }
    assert_equal (1..16).to_a, tickets.map(&:generation)
    assert_equal :full, handoff.start(99, 1, 2, 0, tok(1), 0)
    assert_equal :busy, handoff.start(0, 1, 2, 0, tok(1), 0)
    assert_equal :same_replica, handoff.start(99, 1, 1, 0, tok(1), 0)
    assert_equal rec(:rollback), handoff.abort(tickets.first, nil, 0)
    assert_equal rec(:aborted), handoff.resumed(tickets.first, 1, 0)
    assert_equal 17, handoff.start(99, 1, 2, 0, tok(1), 0).generation
    limited, = begun
    assert_instance_of H::Ticket, limited.start(1, 2, 3, 0, tok(1), 0)
    assert_equal :replica_busy, limited.start(2, 4, 2, 0, tok(1), 0)
    exhausted = H::Handoff.restored(LIMITS, 65_535, 2**64 - 1)
    assert_equal :exhausted, exhausted.start(1, 1, 2, 0, tok(1), 0)
    assert_equal 2**64 - 1, exhausted.generation
  end

  def test_orders_and_overdue_resume_in_bounded_chunks
    handoff = H::Handoff.new(H::Limits.new(5 * SECOND, 16))
    tickets = 5.times.map { |i| handoff.start(i, 1, 2, 0, tok(1), i * SECOND) }
    all = orders(handoff)
    assert_equal tickets, all.map(&:ticket)
    seen = []
    position = 0
    loop do
      chunk, position = handoff.orders(position, 2)
      break if chunk.empty?

      assert_operator chunk.size, :<=, 2
      seen.concat(chunk)
    end
    assert_equal all, seen
    assert_equal [[], 16], handoff.overdue(4 * SECOND, 0, 16)
    assert_equal [tickets.first(3), 16], handoff.overdue(7 * SECOND, 0, 16)
    first, position = handoff.overdue(9 * SECOND, 0, 2)
    assert_equal [tickets.first(2), 2], [first, position]
    assert_equal tickets[2..], handoff.overdue(9 * SECOND, position, 16).first
  end

  def test_time_only_moves_forward
    handoff, ticket = begun
    assert_equal rec(:drain), handoff.quiesced(ticket, 1, 12, 3)
    assert_equal :clock_regressed, handoff.drained(ticket, 1, 10, 2)
    assert_equal :clock_regressed, handoff.abort(ticket, nil, 2)
    assert_equal :clock_regressed, handoff.start(1, 3, 4, 0, tok(1), 2)
    assert_equal rec(:drain), handoff.drained(ticket, 1, 10, 3)
  end

  def test_validation
    handoff, ticket = begun
    [1.0, nil, '1', Float::NAN].each do |bad|
      assert_raises(TypeError) { H::Handoff.new(H::Limits.new(bad, 1)) }
      assert_raises(TypeError) { H::Handoff.restored(LIMITS, 0, bad) }
      assert_raises(TypeError) { H::Handoff.restored(LIMITS, bad, 0) }
      assert_raises(TypeError) { handoff.start(bad, 1, 2, 0, tok(1), 0) }
      assert_raises(TypeError) { handoff.start(1, 1, 2, 0, bad, 0) }
      assert_raises(TypeError) { handoff.quiesced(ticket, 1, 12, bad) }
      assert_raises(TypeError) { handoff.quiesced(bad, 1, 12, 0) }
      assert_raises(TypeError) { handoff.status(bad) }
      assert_raises(TypeError) { handoff.overdue(bad, 0, 1) }
      assert_raises(TypeError) { handoff.orders(0, bad) }
    end
    assert_raises(TypeError) { handoff.abort(ticket, '7', 0) }
    assert_raises(TypeError) { handoff.fenced([9, 1], tok(6), 0) }
    assert_raises(TypeError) { H::Handoff.new([5 * SECOND, 2]) }
    [-1, 2**64].each do |bad|
      assert_raises(RangeError) { handoff.quiesced(ticket, bad, 12, 0) }
      assert_raises(RangeError) { handoff.orders(bad, 1) }
      assert_raises(RangeError) { H::Handoff.new(H::Limits.new(1, bad)) }
    end
    assert_raises(RangeError) { tok(0) }
    assert_raises(ArgumentError) { H::Handoff.new }
    assert_raises(ArgumentError) { handoff.orders }
    assert_raises(ArgumentError) { handoff.abort(ticket, 0) }
    assert_raises(ArgumentError) { H::Limits.new(per_replica: 1) }
    assert_raises(TypeError) { H::Handoff.allocate }
    assert_raises(NoMethodError) { H::Handoff.native_restored(1, 1, [0, 0]) }
    assert_raises(NoMethodError) { handoff.native_abort(9, 1, nil, 0) }
    assert_equal :quiesce, handoff.status(9).phase
  end

  def test_u64_limits_and_freeze
    top = 2**64 - 1
    handoff = H::Handoff.restored(LIMITS, 65_535, top - 1)
    ticket = handoff.start(top, top, 0, top - 1, tok(top), top)
    assert_equal H::Ticket.new(top, top), ticket
    assert_equal top + (5 * SECOND), handoff.status(top).deadline
    assert_equal rec(:drain), handoff.quiesced(ticket, top, top, top)
    assert_equal [top - 1], missing(handoff, ticket)
    # The deadline is past every `now` the coordinator accepts, so it is never overdue.
    assert_equal [[], 16], handoff.overdue(top, 0, 16)
    handoff.freeze
    assert_raises(FrozenError) { handoff.drained(ticket, top, top - 1, top) }
    assert_raises(FrozenError) { handoff.abort(ticket, nil, top) }
    assert_raises(FrozenError) { handoff.start(1, 1, 2, 0, tok(1), top) }
    assert_equal [:drain, LIMITS], [handoff.status(top).phase, handoff.limits]
    assert_equal 1, orders(handoff).size
  end
end
