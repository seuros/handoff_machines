//! `HandoffMachines::Handoff`: the coordinator.
use std::cell::RefCell;

use handoff_machines::{Acked, FencingToken, Refusal};
use magnus::{Error, IntoValue, RArray, Ruby, Value, prelude::*, typed_data::Obj};

use crate::classes::{classes, keep};
use crate::values::{LimitsValue, TicketValue, TokenValue};
use crate::{Core, Number, Scope, names};

type Outcome = Result<Acked, Refusal>;

/// A refusal's symbol.
fn refused(ruby: &Ruby, refusal: Refusal) -> Value {
    ruby.to_symbol(names::refusal(refusal)).as_value()
}

/// A `Recorded` or `Duplicate` when honoured, built by its `new`, or the
/// refusal symbol.
fn acked(ruby: &Ruby, outcome: Outcome) -> Result<Value, Error> {
    let classes = classes(ruby)?;
    let (class, phase) = match outcome {
        Ok(Acked::Recorded(reached)) => (classes.recorded, reached),
        Ok(Acked::Duplicate(stands)) => (classes.duplicate, stands),
        Err(refusal) => return Ok(refused(ruby, refusal)),
    };
    let phase = ruby.to_symbol(names::phase(phase));
    ruby.get_inner(class).funcall("new", (phase,))
}

#[magnus::wrap(class = "HandoffMachines::Handoff", free_immediately, size)]
pub struct Handoff {
    core: RefCell<Core>,
}

impl Handoff {
    pub const fn with_scope(limits: LimitsValue, scope: Scope) -> Self {
        Self {
            core: RefCell::new(Core::with_scope(limits.0, scope.0)),
        }
    }

    pub fn restored(
        ruby: &Ruby,
        limits: LimitsValue,
        scope: Scope,
        last_generation: Number,
    ) -> Result<Self, Error> {
        Core::restored(limits.0, scope.0, last_generation.0)
            .map(|core| Self {
                core: RefCell::new(core),
            })
            .ok_or_else(|| {
                Error::new(
                    ruby.exception_range_error(),
                    "last_generation is not a generation of this scope",
                )
            })
    }

    pub fn scope(&self) -> u16 {
        self.core.borrow().scope()
    }

    pub fn limits(&self) -> (u128, usize) {
        let limits = self.core.borrow().limits();
        (limits.phase_budget().as_nanos(), limits.per_replica())
    }

    /// A `Ticket` when honoured, built by its `new` while the decision is
    /// held on a copy (clone-then-keep), or the refusal symbol.
    #[allow(clippy::too_many_arguments)] // The core's arguments, in order.
    pub fn start(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Number,
        source: Number,
        target: Number,
        base: Number,
        source_token: TokenValue,
        now: Number,
    ) -> Result<Value, Error> {
        let class = classes(ruby)?.ticket;
        let held = source_token.0;
        let start =
            |core: &mut Core| core.start(partition.0, source.0, target.0, base.0, held, now.at());
        keep(ruby, this, &this.core, start, |started| match started {
            Ok(ticket) => ruby
                .get_inner(class)
                .funcall("new", (ticket.partition(), ticket.generation())),
            Err(refusal) => Ok(refused(ruby, refusal)),
        })
    }

    /// Every acknowledgement. Everything that can raise happens before the
    /// core is called; the answer is built while the decision is held on a
    /// copy, kept only once it exists (clone-then-keep).
    fn ack(
        ruby: &Ruby,
        this: Obj<Self>,
        ack: impl FnOnce(&mut Core) -> Outcome,
    ) -> Result<Value, Error> {
        keep(ruby, this, &this.core, ack, |outcome| acked(ruby, outcome))
    }

    pub fn quiesced(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        until: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| {
            core.quiesced(ticket, replica.0, until.0, now.at())
        })
    }

    pub fn drained(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        offset: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| {
            core.drained(ticket, replica.0, offset.0, now.at())
        })
    }

    pub fn committed(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        offset: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| {
            core.committed(ticket, replica.0, offset.0, now.at())
        })
    }

    pub fn captured(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        checkpoint: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| {
            core.captured(ticket, replica.0, checkpoint.0, now.at())
        })
    }

    pub fn installed(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        checkpoint: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| {
            core.installed(ticket, replica.0, checkpoint.0, now.at())
        })
    }

    pub fn fenced(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        token: TokenValue,
        now: Number,
    ) -> Result<Value, Error> {
        let (ticket, token) = (ticket.0, token.0);
        Self::ack(ruby, this, |core| core.fenced(ticket, token, now.at()))
    }

    pub fn activated(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        token: TokenValue,
        now: Number,
    ) -> Result<Value, Error> {
        let (ticket, token) = (ticket.0, token.0);
        Self::ack(ruby, this, |core| {
            core.activated(ticket, replica.0, token, now.at())
        })
    }

    pub fn abort(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        token: Option<TokenValue>,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        let token = token.map(|token| token.0);
        Self::ack(ruby, this, |core| core.abort(ticket, token, now.at()))
    }

    pub fn resumed(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| core.resumed(ticket, replica.0, now.at()))
    }

    pub fn discarded(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: TicketValue,
        replica: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let ticket = ticket.0;
        Self::ack(ruby, this, |core| {
            core.discarded(ticket, replica.0, now.at())
        })
    }

    pub fn status(ruby: &Ruby, this: &Self, partition: Number) -> Option<Value> {
        let status = this.core.borrow().status(partition.0)?;
        let head = (
            status.ticket().partition(),
            status.ticket().generation(),
            status.source(),
            status.target(),
            ruby.to_symbol(names::phase(status.phase())),
        );
        let tail = (
            status.base(),
            status.until(),
            status.checkpoint(),
            status.token().map(FencingToken::raw),
            status.deadline().as_nanos(),
        );
        Some((head, tail).into_value_with(ruby))
    }

    pub fn orders(
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
            let ticket = order.ticket();
            list.push((ticket.partition(), ticket.generation(), to, action, value))?;
        }
        Ok((list, pass.position()))
    }

    pub fn outstanding(
        ruby: &Ruby,
        this: &Self,
        ticket: TicketValue,
        from: Number,
        limit: Number,
    ) -> Result<Value, Error> {
        let core = this.core.borrow();
        let limit = limit.index(ruby)?;
        Ok(match core.outstanding(ticket.0, from.0) {
            Ok(missing) => {
                let list = ruby.ary_new();
                for offset in missing.take(limit) {
                    list.push(offset)?;
                }
                list.as_value()
            }
            Err(refusal) => ruby.to_symbol(names::refusal(refusal)).as_value(),
        })
    }

    pub fn overdue(
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
            list.push((ticket.partition(), ticket.generation()))?;
        }
        Ok((list, pass.position()))
    }

    pub fn active(&self) -> usize {
        self.core.borrow().active()
    }

    pub fn generation(&self) -> u64 {
        self.core.borrow().generation()
    }
}
