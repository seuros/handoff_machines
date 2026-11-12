# frozen_string_literal: true

require_relative 'handoff_machines/version'
# Native is required: no algorithm fallback.
require 'handoff_machines_native/handoff_machines_native'

module HandoffMachines
  # One transfer: the partition and the generation it was issued under.
  # Plain data, so it can travel to a replica and come back in its reply.
  Ticket = Data.define(:partition, :generation)

  # One outstanding instruction. `to` is a replica id or :authority; `value`
  # is the action's operand (quiesce point, checkpoint, token) or nil.
  Order = Data.define(:ticket, :to, :action, :value)

  # A read-only view of one active transfer.
  Status = Data.define(
    :ticket, :source, :target, :phase, :base, :until, :checkpoint, :token, :deadline_ns
  )

  class Coordinator
    def start(partition, source:, target:, base:, token:, now_ns:)
      started = native_start(partition, [source, target], base, token, now_ns)
      started.is_a?(Symbol) ? started : Ticket.new(*started)
    end

    def quiesced(ticket, replica, until_offset, now_ns)
      native_quiesced(*key(ticket), replica, until_offset, now_ns)
    end

    def drained(ticket, replica, offset, now_ns)
      native_drained(*key(ticket), replica, offset, now_ns)
    end

    def committed(ticket, replica, offset, now_ns)
      native_committed(*key(ticket), replica, offset, now_ns)
    end

    def captured(ticket, replica, checkpoint, now_ns)
      native_captured(*key(ticket), replica, checkpoint, now_ns)
    end

    def installed(ticket, replica, checkpoint, now_ns)
      native_installed(*key(ticket), replica, checkpoint, now_ns)
    end

    def fenced(ticket, token, now_ns)
      native_fenced(*key(ticket), token, now_ns)
    end

    def activated(ticket, replica, token, now_ns)
      native_activated(*key(ticket), replica, token, now_ns)
    end

    def abort(ticket, now_ns, token: nil)
      native_abort(*key(ticket), token, now_ns)
    end

    def resumed(ticket, replica, now_ns)
      native_resumed(*key(ticket), replica, now_ns)
    end

    def discarded(ticket, replica, now_ns)
      native_discarded(*key(ticket), replica, now_ns)
    end

    def status(partition)
      head, tail = native_status(partition)
      return unless head

      Status.new(Ticket.new(head[0], head[1]), *head[2..], *tail)
    end

    # At most `limit` orders from position `from`, and the position to resume at.
    def orders(from = 0, limit: CAPACITY * 2)
      rows, position = native_orders(from, limit)
      [rows.map { |partition, generation, *rest| Order.new(Ticket.new(partition, generation), *rest) }, position]
    end

    # At most `limit` offsets the drain still waits for, ascending from `from`.
    def outstanding(ticket, from = 0, limit: WINDOW)
      native_outstanding(*key(ticket), from, limit)
    end

    # At most `limit` overdue tickets from slot `from`, and the slot to resume at.
    def overdue(now_ns, from = 0, limit: CAPACITY)
      rows, position = native_overdue(now_ns, from, limit)
      [rows.map { |row| Ticket.new(*row) }, position]
    end

    private

    def key(ticket)
      raise TypeError, 'expected a HandoffMachines::Ticket' unless ticket.is_a?(Ticket)

      [ticket.partition, ticket.generation]
    end
  end
end
