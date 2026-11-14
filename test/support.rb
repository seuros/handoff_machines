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

# Builds a value past `initialize`: Marshal.load skips it. `from` and `to`
# are the marshalled bytes of one field, before and after.
module Forge
  def forge(value, from, to)
    bytes = Marshal.dump(value)
    raise ArgumentError, 'field not found' unless bytes.include?(from)

    Marshal.load(bytes.sub(from, to)) # rubocop:disable Security/MarshalLoad
  end
end
