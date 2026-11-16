# frozen_string_literal: true

require 'minitest/autorun'
require 'open3'
require 'rbconfig'

# The class of the value the extension wraps (Handoff) is fixed when it loads.
# Nothing a caller does later, redefining the constant or `Object.const_get`,
# reaches a coordinator being built. Each case runs in a fresh process: the
# class is fixed once per process, at load.
class ClassLookupTest < Minitest::Test
  LIB = File.expand_path('../lib', __dir__)

  PRELUDE = <<~RUBY
    require 'handoff_machines'
    H = HandoffMachines
    LIMITS = H::Limits.new(5_000_000_000, 2)
    # Partition 9 handed from replica 1 to 2: the coordinator works.
    def used(handoff) = handoff.start(9, 1, 2, 10, H::FencingToken.from_raw(5), 0).partition
  RUBY

  # Runs `body` after PRELUDE in a new Ruby; answers its last line of output.
  def isolated(body)
    out, err, status = Open3.capture3(RbConfig.ruby, '-I', LIB, '-e', PRELUDE + body)
    assert status.success?, "exited #{status.exitstatus}: #{err}"
    out.lines.last&.chomp
  end

  # Bug: a Handoff constant redefined before the first coordinator made it an
  # instance of the new class, without any of the coordinator's methods.
  def test_a_redefined_handoff_constant_does_not_change_the_handoff_class
    assert_equal 'true 9 true', isolated(<<~RUBY)
      original = H::Handoff
      H.send(:remove_const, :Handoff)
      H.const_set(:Handoff, Class.new)
      handoff = original.new(LIMITS)
      puts [handoff.instance_of?(original), used(handoff),
            original.restored(LIMITS, 0, nil).instance_of?(original)].join(' ')
    RUBY
  end

  # Bug: building the first coordinator ran `Object.const_get`, a caller's
  # Ruby code in the middle of a native call.
  def test_building_a_handoff_runs_no_const_get
    assert_equal '0 Handoff Handoff', isolated(<<~RUBY)
      hits = 0
      Object.define_singleton_method(:const_get) do |*args|
        hits += 1
        super(*args)
      end
      handoffs = [H::Handoff.with_scope(LIMITS, 3), H::Handoff.restored(LIMITS, 0, nil)]
      puts [hits, *handoffs.map { |handoff| handoff.class.name.split('::').last }].join(' ')
    RUBY
  end

  # Bug: a redefined `Object.const_get` that raised hit an `unwrap` in the
  # extension: an uncatchable fatal, and the process exited.
  def test_a_raising_const_get_does_not_kill_the_process
    assert_equal '9', isolated(<<~RUBY)
      Object.define_singleton_method(:const_get) { |*| raise IOError, 'lookup ran mid-call' }
      puts used(H::Handoff.new(LIMITS))
    RUBY
  end
end
