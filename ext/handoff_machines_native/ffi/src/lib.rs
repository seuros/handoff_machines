//! Thin binding to the bounded partition handoff coordinator.
mod init;
mod names;

use std::{cell::RefCell, time::Duration};

use handoff_machines::{Acked, FencingToken, Limits, Refusal, Ticket};
use magnus::{
    Error, Integer, IntoValue, RArray, Ruby, TryConvert, Value, prelude::*, typed_data::Obj,
};

/// Concurrent transfers compiled into the binding.
const CAPACITY: usize = 16;
/// Drain window words: 64 offsets each.
const WORDS: usize = 4;

type Core = handoff_machines::Handoff<CAPACITY, WORDS>;
type Outcome = Result<Acked, Refusal>;

#[derive(Clone, Copy)]
struct Number(u64);

impl TryConvert for Number {
    fn try_convert(value: Value) -> Result<Self, Error> {
        if Integer::from_value(value).is_none() {
            return Err(Error::new(
                Ruby::get_with(value).exception_type_error(),
                "expected an Integer (unsigned 64-bit)",
            ));
        }
        u64::try_convert(value).map(Self)
    }
}

impl Number {
    const fn at(self) -> Duration {
        Duration::from_nanos(self.0)
    }

    fn token(self, ruby: &Ruby) -> Result<FencingToken, Error> {
        FencingToken::new(self.0)
            .ok_or_else(|| Error::new(ruby.exception_range_error(), "fencing tokens are positive"))
    }

    fn index(self, ruby: &Ruby) -> Result<usize, Error> {
        usize::try_from(self.0)
            .map_err(|_| Error::new(ruby.exception_range_error(), "integer too large"))
    }
}

const fn ticket(partition: Number, generation: Number) -> Ticket {
    Ticket {
        partition: partition.0,
        generation: generation.0,
    }
}

#[magnus::wrap(class = "HandoffMachines::Handoff", free_immediately, size)]
struct Handoff {
    core: RefCell<Core>,
}

impl Handoff {
    fn restored(
        ruby: &Ruby,
        phase_budget: Number,
        per_replica: Number,
        last_generation: Number,
    ) -> Result<Self, Error> {
        let limits = Limits {
            phase_budget: phase_budget.at(),
            per_replica: per_replica.index(ruby)?,
        };
        Ok(Self {
            core: RefCell::new(Core::restored(limits, last_generation.0)),
        })
    }

    fn limits(&self) -> (u128, usize) {
        let limits = self.core.borrow().limits();
        (limits.phase_budget.as_nanos(), limits.per_replica)
    }

    fn start(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        replicas: (Number, Number),
        base: Number,
        source_token: Number,
        now: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let held = source_token.token(ruby)?;
        let (source, target) = replicas;
        let started =
            this.core
                .borrow_mut()
                .start(partition.0, source.0, target.0, base.0, held, now.at());
        Ok(match started {
            Ok(ticket) => (ticket.partition, ticket.generation).into_value_with(ruby),
            Err(refusal) => ruby.to_symbol(names::refusal(refusal)).as_value(),
        })
    }

    /// Every acknowledgement: `[duplicate, phase]` when honoured, or the
    /// refusal symbol.
    fn ack(
        ruby: &Ruby,
        this: Obj<Self>,
        ack: impl FnOnce(&mut Core) -> Outcome,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = ack(&mut this.core.borrow_mut());
        Ok(names::acked(ruby, outcome))
    }

    fn quiesced(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        until: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| {
            core.quiesced(ticket, replica.0, until.0, now.at())
        })
    }

    fn drained(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        offset: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| {
            core.drained(ticket, replica.0, offset.0, now.at())
        })
    }

    fn committed(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        offset: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| {
            core.committed(ticket, replica.0, offset.0, now.at())
        })
    }

    fn captured(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        checkpoint: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| {
            core.captured(ticket, replica.0, checkpoint.0, now.at())
        })
    }

    fn installed(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        checkpoint: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| {
            core.installed(ticket, replica.0, checkpoint.0, now.at())
        })
    }

    fn fenced(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        token: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let (ticket, token) = (ticket(partition, generation), token.token(ruby)?);
        Self::ack(ruby, this, |core| core.fenced(ticket, token, now.at()))
    }

    fn activated(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        token: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let (ticket, token) = (ticket(partition, generation), token.token(ruby)?);
        Self::ack(ruby, this, |core| {
            core.activated(ticket, replica.0, token, now.at())
        })
    }

    fn abort(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        token: Option<Number>,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        let token = token.map(|token| token.token(ruby)).transpose()?;
        Self::ack(ruby, this, |core| core.abort(ticket, token, now.at()))
    }

    fn resumed(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| core.resumed(ticket, replica.0, now.at()))
    }

    fn discarded(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        generation: Number,
        replica: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket(partition, generation);
        Self::ack(ruby, this, |core| {
            core.discarded(ticket, replica.0, now.at())
        })
    }

    fn status(ruby: &Ruby, this: &Self, partition: Number) -> Option<Value> {
        let status = this.core.borrow().status(partition.0)?;
        let head = (
            status.ticket.partition,
            status.ticket.generation,
            status.source,
            status.target,
            ruby.to_symbol(names::phase(status.phase)),
        );
        let tail = (
            status.base,
            status.until,
            status.checkpoint,
            status.token.map(FencingToken::value),
            status.deadline.as_nanos(),
        );
        Some((head, tail).into_value_with(ruby))
    }

    fn orders(
        ruby: &Ruby,
        this: &Self,
        from: Number,
        limit: Number,
    ) -> Result<(RArray, usize), Error> {
        let core = this.core.borrow();
        let mut pass = core.orders(from.index(ruby)?);
        let list = ruby.ary_new();
        for order in pass.by_ref().take(limit.index(ruby)?) {
            let (to, action, value) = names::order(ruby, &order);
            let ticket = order.ticket;
            list.push((ticket.partition, ticket.generation, to, action, value))?;
        }
        Ok((list, pass.position()))
    }

    fn outstanding(
        ruby: &Ruby,
        this: &Self,
        partition: Number,
        generation: Number,
        from: Number,
        limit: Number,
    ) -> Result<Value, Error> {
        let core = this.core.borrow();
        let limit = limit.index(ruby)?;
        Ok(
            match core.outstanding(ticket(partition, generation), from.0) {
                Ok(missing) => {
                    let list = ruby.ary_new();
                    for offset in missing.take(limit) {
                        list.push(offset)?;
                    }
                    list.as_value()
                }
                Err(refusal) => ruby.to_symbol(names::refusal(refusal)).as_value(),
            },
        )
    }

    fn overdue(
        ruby: &Ruby,
        this: &Self,
        now: Number,
        from: Number,
        limit: Number,
    ) -> Result<(RArray, usize), Error> {
        let core = this.core.borrow();
        let mut pass = core.overdue(now.at(), from.index(ruby)?);
        let list = ruby.ary_new();
        for ticket in pass.by_ref().take(limit.index(ruby)?) {
            list.push((ticket.partition, ticket.generation))?;
        }
        Ok((list, pass.position()))
    }

    fn active(&self) -> usize {
        self.core.borrow().active()
    }

    fn generation(&self) -> u64 {
        self.core.borrow().generation()
    }
}
