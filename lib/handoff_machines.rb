# frozen_string_literal: true

require_relative 'handoff_machines/version'
# Native is required: no algorithm fallback.
require 'handoff_machines_native/handoff_machines_native'

# Every value mirrors the core type of the same name, field for field. Every
# instant and span (`now`, `phase_budget`, `deadline`) is Integer nanoseconds.
module HandoffMachines
  # Admission and timing limits: how long one phase may last before `overdue`
  # reports the transfer, and how many transfers one replica may take part in.
  Limits = Data.define(:phase_budget, :per_replica)

  # One transfer: the partition and the generation it was issued under.
  # Plain data, so it can travel to a replica and come back in its reply.
  Ticket = Data.define(:partition, :generation)

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
    private_constant :ACTIONS

    class << self
      def new(limits)
        restored(limits, 0)
      end

      def restored(limits, last_generation)
        raise TypeError, 'expected a HandoffMachines::Limits' unless limits.is_a?(Limits)

        native_restored(limits.phase_budget, limits.per_replica, last_generation)
      end

      private :native_restored
    end

    def limits
      Limits.new(*native_limits)
    end

    def start(partition, source, target, base, source_token, now)
      started = native_start(partition, [source, target], base, source_token, now)
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
      acked(native_fenced(*key(ticket), token, now))
    end

    def activated(ticket, replica, token, now)
      acked(native_activated(*key(ticket), replica, token, now))
    end

    # `token` is nil for an unfenced abort.
    def abort(ticket, token, now)
      acked(native_abort(*key(ticket), token, now))
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

      Status.new(Ticket.new(head[0], head[1]), *head[2..], *tail)
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

    def acked(answer)
      return answer if answer.is_a?(Symbol)

      duplicate, phase = answer
      (duplicate ? Duplicate : Recorded).new(phase)
    end

    def order(partition, generation, recipient, name, field)
      to = recipient.is_a?(Symbol) ? recipient : Replica.new(recipient)
      action = ACTIONS.key?(name) ? ACTIONS.fetch(name).new(field) : name
      Order.new(Ticket.new(partition, generation), to, action)
    end
  end
end
