# frozen_string_literal: true

require_relative 'handoff_machines/version'

# Every value mirrors the core type of the same name, field for field. Every
# instant and span (`now`, `phase_budget`, `deadline`) is Integer nanoseconds.
#
# The native side is the authority on every value it reads: one of this gem's
# classes must be exactly that class (a subclass is a TypeError, whatever it
# holds), and every field is validated again through the core constructor, so
# a value forged past `initialize` (`Marshal.load`, a bound
# `Data#initialize`) raises TypeError or RangeError and never reaches the core.
module HandoffMachines

  # A fencing token, the lease core's `FencingToken` which the handoff core
  # re-exports: the issuing authority's scope in the top 16 bits of `raw`
  # above a sequence number counted from 1. The coordinator issues none: bring
  # them from your authority. Every method that takes a token reads its `raw`,
  # so the `lease_machines` gem's FencingToken fits as it is, and this gem does
  # not depend on that one; a value that keeps a token (Fence, Activate,
  # Resume, Status) converts it to this class. A bare Integer is not a token:
  # TypeError.
  #
  # Tokens of one scope are ordered; tokens of two scopes are not, so there is
  # no `<` or `<=>`. Compare with `order` and `newer_than?`.
  FencingToken = Data.define(:raw) do
    # The core's `FencingToken::from_raw`. RangeError when the sequence number
    # is zero, which no authority issues.
    def self.from_raw(raw) = new(raw)

    def initialize(raw:)
      HandoffMachines.__send__(:native_raw, raw)
      super
    end

    # The scope of the authority that issued this token.
    def scope = HandoffMachines.__send__(:native_token_scope, self)

    # -1, 0 or 1 against a token of the same scope; nil across scopes.
    def order(other) = HandoffMachines.__send__(:native_token_order, self, other)

    # Whether this token is of `other`'s scope and strictly newer. False across
    # scopes, so its negation does not mean "older or equal".
    def newer_than?(other) = HandoffMachines.__send__(:native_token_newer, self, other)
  end
  # Every value below is valid or does not exist: it is checked at
  # construction through the core constructor of the type it mirrors.
  # TypeError for a value of the wrong type, RangeError for one the core would
  # not build. A value built by hand is held to the same rule as one returned.

  # Admission and timing limits: how long one phase may last before `overdue`
  # reports the transfer, and how many transfers one replica may take part in.
  # The core's `Limits::new`: `per_replica` is at least 1.
  Limits = Data.define(:phase_budget, :per_replica) do
    def initialize(phase_budget:, per_replica:)
      HandoffMachines.__send__(:__limits, phase_budget, per_replica)
      super
    end
  end

  # One transfer: the partition and the generation it was issued under.
  # Plain data, so it can travel to a replica and come back in its reply. The
  # generation carries the scope of the coordinator that issued it; a
  # coordinator of another scope answers :unknown to the ticket. The core's
  # `Ticket::new`: a generation with a zero sequence, which no coordinator
  # issues, raises RangeError.
  Ticket = Data.define(:partition, :generation) do
    def initialize(partition:, generation:)
      HandoffMachines.__send__(:__ticket, partition, generation)
      super
    end

    # The scope of the coordinator that issued this ticket.
    def scope = Handoff.__send__(:native_scope_of, self)
  end

  # Honoured acknowledgement, recorded for the first time; the transfer now
  # stands at `phase`. The core's `Acked::recorded`: never :quiesce.
  Recorded = Data.define(:phase) do
    def initialize(phase:)
      HandoffMachines.__send__(:__recorded, phase)
      super
    end

    def duplicate? = false
  end

  # Honoured idempotent repeat: nothing changed, and the transfer still stands
  # at `phase`, which may be later than the one the acknowledgement first led
  # to. The core's `Acked::duplicate`: never :quiesce, :complete or :aborted.
  Duplicate = Data.define(:phase) do
    def initialize(phase:)
      HandoffMachines.__send__(:__duplicate, phase)
      super
    end

    def duplicate? = true
  end

  # Order#to for a replica, by its caller-owned id. The lease authority is
  # the symbol :authority.
  Replica = Data.define(:id) do
    def initialize(id:)
      HandoffMachines.__send__(:__u64, id)
      super
    end
  end

  # Order#action, one Data per core `Action` variant that carries a field.
  # The two that carry none are the symbols :quiesce and :discard, and only
  # those two are Symbols: a bare :resume is not `Resume.new(nil)`. Offsets
  # and checkpoint ids are Integers in 0...2**64; tokens are converted to
  # this gem's FencingToken.
  Drain = Data.define(:until) do
    def initialize(**field)
      HandoffMachines.__send__(:__u64, field[:until]) if field.key?(:until)
      super
    end
  end
  Capture = Data.define(:at) do
    def initialize(at:)
      HandoffMachines.__send__(:__u64, at)
      super
    end
  end
  Install = Data.define(:checkpoint) do
    def initialize(checkpoint:)
      HandoffMachines.__send__(:__u64, checkpoint)
      super
    end
  end
  Fence = Data.define(:above) do
    def initialize(above:) = super(above: HandoffMachines.__send__(:__token, above))
  end
  Activate = Data.define(:token) do
    def initialize(token:) = super(token: HandoffMachines.__send__(:__token, token))
  end
  # `token` is nil when the rollback was not fenced.
  Resume = Data.define(:token) do
    def initialize(token:) = super(token: token && HandoffMachines.__send__(:__token, token))
  end

  # One outstanding instruction: who must act (`to`, a Replica or
  # :authority) and what they must do. The core's `Order::new`: only the
  # authority is asked to fence, and it is asked nothing else.
  Order = Data.define(:ticket, :to, :action) do
    def initialize(ticket:, to:, action:)
      HandoffMachines.__send__(:__order, ticket, to, action)
      super
    end
  end

  # A read-only view of one active transfer. The core's `Status::checked`:
  # a ticket some coordinator issues, two different replicas, a phase that is
  # not final, and only what that phase has recorded.
  Status = Data.define(
    :ticket, :source, :target, :phase, :base, :until, :checkpoint, :token, :deadline
  ) do
    # `until` is a keyword, so the fields arrive as a Hash. A missing or
    # unknown one is Data's own ArgumentError.
    def initialize(**fields)
      return super unless fields.keys.sort == members.sort

      super(**fields.merge(token: HandoffMachines.__send__(:__status, *fields.values_at(*members))))
    end
  end

  # Sealed: no subclass can skip the checks above by overriding `initialize`,
  # and no bare `allocate` builds a value that never ran them. Ruby still
  # binds a subclass constant before `inherited` raises; the native side
  # refuses its instances.
  [FencingToken, Limits, Ticket, Recorded, Duplicate, Replica, Drain, Capture, Install, Fence, Activate, Resume,
   Order, Status].each do |value|
    value.singleton_class.undef_method(:allocate)
    value.define_singleton_method(:inherited) { |_| raise TypeError, "#{value} cannot be subclassed" }
  end

  # The action Data a native row names, by the action's name.
  ACTIONS = { drain: Drain, capture: Capture, install: Install, fence: Fence, activate: Activate,
              resume: Resume }.freeze
  # The actions whose field is a fencing token.
  TOKENED = %i[fence activate resume].freeze
  private_constant :ACTIONS, :TOKENED
end

# Native is required: no algorithm fallback. It resolves the classes above
# once, here.
require 'handoff_machines_native/handoff_machines_native'

module HandoffMachines
  class << self
    private :native_raw, :native_token_scope, :native_token_order, :native_token_newer,
            :__limits, :__ticket, :__recorded, :__duplicate, :__u64, :__token, :__order, :__status
  end

  # The core's `Handoff`. Every mutating call (start and each
  # acknowledgement) reads its arguments before the core is asked, decides on
  # a copy, builds the answer (a Ticket, a Recorded or Duplicate: their `new`
  # runs), then keeps the copy only if this coordinator is still unfrozen
  # (FrozenError) and unchanged (RuntimeError: a re-entrant call changed it).
  # Either way, a raise means nothing was applied. A refusal is a Symbol.
  class Handoff
    class << self
      # A fresh generation namespace in scope zero.
      def new(limits) = with_scope(limits, 0)

      # A fresh generation namespace whose generations carry `scope`, an
      # Integer in 0..65_535. Give each coordinator its own.
      def with_scope(limits, scope) = native_with_scope(limits, scope)

      # Generations of `scope` above `last_generation`: what #generation
      # answered on the previous coordinator of this namespace. RangeError when
      # `last_generation` is of another scope, a bare 0 for a non-zero scope
      # included: with nothing saved, call with_scope.
      def restored(limits, scope, last_generation) = native_restored(limits, scope, last_generation)

      private :native_with_scope, :native_restored, :native_scope_of
    end

    def limits
      Limits.new(*native_limits)
    end

    def status(partition)
      head, tail = native_status(partition)
      return unless head

      granted = tail[3] && FencingToken.new(tail[3])
      Status.new(Ticket.new(head[0], head[1]), *head[2..], *tail[0..2], granted, tail[4])
    end

    # At most `limit` orders from position `from` (zero to begin), and the
    # position to resume at.
    def orders(from, limit)
      rows, position = native_orders(from, limit)
      [rows.map { |row| order(*row) }, position]
    end

    # At most `limit` overdue tickets from slot `from` (zero to begin), and the
    # slot to resume at.
    def overdue(now, from, limit)
      rows, position = native_overdue(now, from, limit)
      [rows.map { |row| Ticket.new(*row) }, position]
    end

    private

    def order(partition, generation, recipient, name, field)
      to = recipient.is_a?(Symbol) ? recipient : Replica.new(recipient)
      Order.new(Ticket.new(partition, generation), to, action(name, field))
    end

    # The action Data or Symbol a native row names.
    def action(name, field)
      return name unless ACTIONS.key?(name)

      ACTIONS.fetch(name).new(field && TOKENED.include?(name) ? FencingToken.new(field) : field)
    end
  end
end
