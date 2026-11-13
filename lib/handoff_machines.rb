# frozen_string_literal: true

require_relative 'handoff_machines/version'
# Native is required: no algorithm fallback.
require 'handoff_machines_native/handoff_machines_native'

# Every value mirrors the core type of the same name, field for field. Every
# instant and span (`now`, `phase_budget`, `deadline`) is Integer nanoseconds.
module HandoffMachines
  class << self
    private :native_token_scope, :native_token_order, :native_token_newer
  end

  # A fencing token, the lease core's `FencingToken` which the handoff core
  # re-exports: the issuing authority's scope in the top 16 bits of `raw`
  # above a sequence number counted from 1. The coordinator issues none: bring
  # them from your authority. Every method that takes a token reads its `raw`,
  # so the `lease_machines` gem's FencingToken fits as it is, and this gem does
  # not depend on that one. A bare Integer is not a token: TypeError.
  #
  # Tokens of one scope are ordered; tokens of two scopes are not, so there is
  # no `<` or `<=>`. Compare with `order` and `newer_than?`.
  FencingToken = Data.define(:raw) do
    # The core's `FencingToken::from_raw`. RangeError when the sequence number
    # is zero, which no authority issues.
    def self.from_raw(raw) = new(raw)

    def initialize(raw:)
      HandoffMachines.__send__(:native_token_scope, raw)
      super
    end

    # The scope of the authority that issued this token.
    def scope = HandoffMachines.__send__(:native_token_scope, raw)

    # -1, 0 or 1 against a token of the same scope; nil across scopes.
    def order(other) = HandoffMachines.__send__(:native_token_order, raw, Rows.raw(other))

    # Whether this token is of `other`'s scope and strictly newer. False across
    # scopes, so its negation does not mean "older or equal".
    def newer_than?(other) = HandoffMachines.__send__(:native_token_newer, raw, Rows.raw(other))
  end
  # What the native extension trades in: a token is read by its `raw`,
  # whichever gem defined it.
  module Rows
    module_function

    def raw(token)
      raise TypeError, 'expected a fencing token (no #raw)' unless token.respond_to?(:raw)

      token.raw
    end
  end
  private_constant :Rows

  # Admission and timing limits: how long one phase may last before `overdue`
  # reports the transfer, and how many transfers one replica may take part in.
  Limits = Data.define(:phase_budget, :per_replica)

  # One transfer: the partition and the generation it was issued under.
  # Plain data, so it can travel to a replica and come back in its reply. The
  # generation carries the scope of the coordinator that issued it; a
  # coordinator of another scope answers :unknown to the ticket.
  Ticket = Data.define(:partition, :generation) do
    # The scope of the coordinator that issued this ticket.
    def scope = Handoff.__send__(:native_scope_of, generation)
  end

  # Honoured acknowledgement, recorded for the first time; the transfer now
  # stands at `phase`.
  Recorded = Data.define(:phase) do
    def duplicate? = false
  end

  # Honoured idempotent repeat: nothing changed, and the transfer still stands
  # at `phase`, which may be later than the one the acknowledgement first led to.
  Duplicate = Data.define(:phase) do
    def duplicate? = true
  end

  # Order#to for a replica, by its caller-owned id. The lease authority is
  # the symbol :authority.
  Replica = Data.define(:id)

  # Order#action, one Data per core `Action` variant that carries a field.
  # The two that carry none are the symbols :quiesce and :discard.
  Drain = Data.define(:until)
  Capture = Data.define(:at)
  Install = Data.define(:checkpoint)
  Fence = Data.define(:above)
  Activate = Data.define(:token)
  Resume = Data.define(:token)

  # One outstanding instruction: who must act (`to`) and what they must do.
  Order = Data.define(:ticket, :to, :action)

  # A read-only view of one active transfer.
  Status = Data.define(
    :ticket, :source, :target, :phase, :base, :until, :checkpoint, :token, :deadline
  )

  class Handoff
    ACTIONS = {
      drain: Drain, capture: Capture, install: Install, fence: Fence, activate: Activate, resume: Resume
    }.freeze
    # The actions whose field is a fencing token.
    TOKENED = %i[fence activate resume].freeze
    private_constant :ACTIONS, :TOKENED

    class << self
      # A fresh generation namespace in scope zero.
      def new(limits) = with_scope(limits, 0)

      # A fresh generation namespace whose generations carry `scope`, an
      # Integer in 0..65_535. Give each coordinator its own.
      def with_scope(limits, scope)
        native_with_scope(*fields(limits), scope)
      end

      # Generations of `scope` above `last_generation`: what #generation
      # answered on the previous coordinator of this namespace. RangeError when
      # `last_generation` is of another scope, a bare 0 for a non-zero scope
      # included: with nothing saved, call with_scope.
      def restored(limits, scope, last_generation)
        native_restored(*fields(limits), [scope, last_generation])
      end

      private

      def fields(limits)
        raise TypeError, 'expected a HandoffMachines::Limits' unless limits.is_a?(Limits)

        [limits.phase_budget, limits.per_replica]
      end

      private :native_with_scope, :native_restored, :native_scope_of
    end

    def limits
      Limits.new(*native_limits)
    end

    def start(partition, source, target, base, source_token, now)
      started = native_start(partition, [source, target], base, raw(source_token), now)
      started.is_a?(Symbol) ? started : Ticket.new(*started)
    end

    def quiesced(ticket, replica, until_offset, now)
      acked(native_quiesced(*key(ticket), replica, until_offset, now))
    end

    def drained(ticket, replica, offset, now)
      acked(native_drained(*key(ticket), replica, offset, now))
    end

    def committed(ticket, replica, offset, now)
      acked(native_committed(*key(ticket), replica, offset, now))
    end

    def captured(ticket, replica, checkpoint, now)
      acked(native_captured(*key(ticket), replica, checkpoint, now))
    end

    def installed(ticket, replica, checkpoint, now)
      acked(native_installed(*key(ticket), replica, checkpoint, now))
    end

    def fenced(ticket, token, now)
      acked(native_fenced(*key(ticket), raw(token), now))
    end

    def activated(ticket, replica, token, now)
      acked(native_activated(*key(ticket), replica, raw(token), now))
    end

    # `token` is nil for an unfenced abort.
    def abort(ticket, token, now)
      acked(native_abort(*key(ticket), token && raw(token), now))
    end

    def resumed(ticket, replica, now)
      acked(native_resumed(*key(ticket), replica, now))
    end

    def discarded(ticket, replica, now)
      acked(native_discarded(*key(ticket), replica, now))
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

    # At most `limit` offsets the drain still waits for, ascending from `from`.
    def outstanding(ticket, from, limit)
      native_outstanding(*key(ticket), from, limit)
    end

    # At most `limit` overdue tickets from slot `from` (zero to begin), and the
    # slot to resume at.
    def overdue(now, from, limit)
      rows, position = native_overdue(now, from, limit)
      [rows.map { |row| Ticket.new(*row) }, position]
    end

    private

    def key(ticket)
      raise TypeError, 'expected a HandoffMachines::Ticket' unless ticket.is_a?(Ticket)

      [ticket.partition, ticket.generation]
    end

    def raw(token) = Rows.raw(token)

    def acked(answer)
      return answer if answer.is_a?(Symbol)

      duplicate, phase = answer
      (duplicate ? Duplicate : Recorded).new(phase)
    end

    def order(partition, generation, recipient, name, field)
      to = recipient.is_a?(Symbol) ? recipient : Replica.new(recipient)
      field = FencingToken.new(field) if field && TOKENED.include?(name)
      action = ACTIONS.key?(name) ? ACTIONS.fetch(name).new(field) : name
      Order.new(Ticket.new(partition, generation), to, action)
    end
  end
end
