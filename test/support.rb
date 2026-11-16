# frozen_string_literal: true

# Runs a hook once in place of the next `new` on a class, before the real
# one: an answer class whose construction raises, re-enters or freezes.
module AnswerHook
  HOOKS = {} # rubocop:disable Style/MutableConstant

  def new(...)
    AnswerHook::HOOKS.delete(self)&.call
    super
  end

  def self.around(klass, hook)
    klass.singleton_class.prepend(self) unless klass.singleton_class.ancestors.include?(self)
    HOOKS[klass] = hook
    yield
  ensure
    HOOKS.delete(klass)
  end
end

# Swaps methods on the gem's classes for the duration of a block.
module Redefining
  private

  # Runs the block with each method of `klass` named in `bodies` replaced by
  # its body, then restores the originals.
  def redefining(klass, **bodies)
    originals = bodies.to_h { |name, _| [name, klass.instance_method(name)] }
    bodies.each { |name, body| replace_method(klass, name, body) }
    yield
  ensure
    originals&.each { |name, method| replace_method(klass, name, (method if method.owner == klass)) }
  end

  # Defines `name` on `klass` as `body` (removes it when `body` is nil),
  # removing the method `klass` itself holds first. Ruby warns about the
  # redefinition (and about removing `initialize`), deliberate here, so
  # warnings are off for the swap alone.
  def replace_method(klass, name, body)
    verbose = $VERBOSE
    $VERBOSE = nil
    klass.remove_method(name) if klass.method_defined?(name, false) || klass.private_method_defined?(name, false)
    klass.define_method(name, body) if body
  ensure
    $VERBOSE = verbose
  end
end

# Builds a value past `initialize`: Marshal.load skips it. `from` and `to`
# are the marshalled bytes of one field, before and after.
module Forge
  def forge(value, from, to)
    bytes = Marshal.dump(value)
    raise ArgumentError, 'field not found' unless bytes.include?(from)

    Marshal.load(bytes.sub(from, to)) # rubocop:disable Security/MarshalLoad
  end
end
