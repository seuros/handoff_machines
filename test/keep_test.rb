# frozen_string_literal: true

require 'minitest/autorun'
require 'handoff_machines'
require_relative 'support'

# Building an answer runs Ruby (`new`), so a decision is held on a copy and
# kept only once its answer exists: whatever happens in between, a raise
# means nothing was applied.
class KeepTest < Minitest::Test
  H = HandoffMachines
  LIMITS = H::Limits.new(5_000_000_000, 2)

  def tok(raw) = H::FencingToken.from_raw(raw)
  def snapshot(handoff) = [handoff.status(9), handoff.status(3), handoff.orders(0, 32), handoff.generation]

  def setup
    @handoff = H::Handoff.new(LIMITS)
    @ticket = @handoff.start(9, 1, 2, 10, tok(5), 0)
  end

  def test_an_answer_whose_new_raises_applies_nothing
    [[H::Ticket, -> { @handoff.start(3, 3, 4, 0, tok(1), 1) }],
     [H::Recorded, -> { @handoff.quiesced(@ticket, 1, 12, 1) }]].each do |answer, call|
      before = snapshot(@handoff)
      AnswerHook.around(answer, -> { raise IOError, 'interrupted' }) { assert_raises(IOError) { call.call } }
      assert_equal before, snapshot(@handoff)
    end
    assert_equal H::Recorded.new(:drain), @handoff.quiesced(@ticket, 1, 12, 1)
  end

  def test_an_answer_whose_new_re_enters_applies_only_the_inner_call
    inner = nil
    AnswerHook.around(H::Recorded, -> { inner = @handoff.quiesced(@ticket, 1, 10, 1) }) do
      assert_raises(RuntimeError) { @handoff.quiesced(@ticket, 1, 12, 1) }
    end
    assert_equal H::Recorded.new(:checkpoint), inner
    assert_equal [:checkpoint, 10], [@handoff.status(9).phase, @handoff.status(9).until]
  end

  def test_an_answer_whose_new_freezes_the_receiver_applies_nothing
    before = snapshot(@handoff)
    AnswerHook.around(H::Ticket, -> { @handoff.freeze }) do
      assert_raises(FrozenError) { @handoff.start(3, 3, 4, 0, tok(1), 1) }
    end
    assert_equal before, snapshot(@handoff)
    assert_raises(FrozenError) { @handoff.quiesced(@ticket, 1, 12, 1) }
  end

  # Frozen is checked before any argument is read: a frozen coordinator
  # handed a malformed argument raises FrozenError, as Ruby's own core
  # methods do.
  def test_frozen_is_checked_before_the_arguments
    @handoff.freeze
    assert_raises(FrozenError) { @handoff.start(1.5, 3, 4, 0, tok(1), 1) }
    assert_raises(FrozenError) { @handoff.start(3, 3, 4, 0, :token, 1) }
    assert_raises(FrozenError) { @handoff.quiesced(nil, 1, 12, 1) }
    assert_raises(FrozenError) { @handoff.drained(@ticket, -1, 12, 1) }
    assert_raises(FrozenError) { @handoff.committed(@ticket, 1, '12', 1) }
    assert_raises(FrozenError) { @handoff.captured(@ticket, 1, 7, nil) }
    assert_raises(FrozenError) { @handoff.installed(@ticket, 2, 2**64, 1) }
    assert_raises(FrozenError) { @handoff.fenced(@ticket, 6, 1) }
    assert_raises(FrozenError) { @handoff.activated(@ticket, 2, nil, 1) }
    assert_raises(FrozenError) { @handoff.abort(@ticket, :token, 1) }
    assert_raises(FrozenError) { @handoff.resumed(Object.new, 1, 1) }
    assert_raises(FrozenError) { @handoff.discarded(@ticket, 2, 1.0) }
  end

  # The binding holds its classes by address: they are pinned, so compaction
  # moves none of them and every path that checks or builds one still works.
  def test_classes_survive_compaction
    GC.compact if GC.respond_to?(:compact)
    assert_instance_of H::Ticket, @handoff.start(3, 3, 4, 0, tok(1), 1)
    assert_equal H::Recorded.new(:drain), @handoff.quiesced(@ticket, 1, 12, 1)
    assert_equal H::Duplicate.new(:drain), @handoff.quiesced(@ticket, 1, 12, 1)
    assert_instance_of H::Handoff, H::Handoff.with_scope(LIMITS, 1)
    order = H::Order.new(@ticket, H::Replica.new(1), H::Drain.new(12))
    assert_equal [order], @handoff.orders(0, 32).first.select { |o| o.ticket == @ticket }
    assert_instance_of H::FencingToken, H::Fence.new(tok(6)).above
    assert_instance_of H::Status, @handoff.status(9)
  end
end
