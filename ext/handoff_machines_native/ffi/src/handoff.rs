//! `HandoffMachines::Handoff`: the coordinator.
use std::cell::RefCell;

use std::time::Duration;

use handoff_machines::{Acked, FencingToken, Refusal, Ticket};
use magnus::{Error, Integer, IntoValue, RArray, Ruby, Value, prelude::*, typed_data::Obj};
use matryona_machines::ruby::{Symbols, duration, integer, keep, nanoseconds};

use crate::classes::classes;
use crate::values;
use crate::{Core, names};

type Outcome = Result<Acked, Refusal>;

/// A ticket, a replica and an offset or checkpoint, converted in that order.
fn report(
    ruby: &Ruby,
    (ticket, replica, value, now): (Value, Value, Value, Value),
    name: &str,
) -> Result<(Ticket, u64, u64, Duration), Error> {
    let (ticket, replica) = (values::ticket(ruby, ticket)?, integer(replica, "replica")?);
    Ok((
        ticket,
        replica,
        integer(value, name)?,
        duration(now, "now")?,
    ))
}

/// A ticket, a replica and a `now`, converted in that order.
fn settle(
    ruby: &Ruby,
    ticket: Value,
    replica: Value,
    now: Value,
) -> Result<(Ticket, u64, Duration), Error> {
    let (ticket, replica) = (values::ticket(ruby, ticket)?, integer(replica, "replica")?);
    Ok((ticket, replica, duration(now, "now")?))
}

/// A refusal's symbol.
fn refused(ruby: &Ruby, refusal: Refusal) -> Value {
    names::Refusals::symbol(ruby, refusal).as_value()
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
    let phase = names::Phases::symbol(ruby, phase);
    class.get(ruby).funcall("new", (phase,))
}

#[magnus::wrap(class = "HandoffMachines::Handoff", free_immediately, size)]
pub struct Handoff {
    core: RefCell<Core>,
}

impl Handoff {
    pub fn with_scope(ruby: &Ruby, limits: Value, scope: Value) -> Result<Self, Error> {
        let (limits, scope) = (values::limits(ruby, limits)?, integer(scope, "scope")?);
        Ok(Self {
            core: RefCell::new(Core::with_scope(limits, scope)),
        })
    }

    pub fn restored(
        ruby: &Ruby,
        limits: Value,
        scope: Value,
        last_generation: Value,
    ) -> Result<Self, Error> {
        let (limits, scope) = (values::limits(ruby, limits)?, integer(scope, "scope")?);
        let last_generation = integer(last_generation, "last_generation")?;
        Core::restored(limits, scope, last_generation)
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

    pub fn limits(ruby: &Ruby, this: &Self) -> (Integer, usize) {
        let limits = this.core.borrow().limits();
        (
            nanoseconds(ruby, limits.phase_budget()),
            limits.per_replica(),
        )
    }

    /// A `Ticket` when honoured, built by its `new` while the decision is
    /// held on a copy (clone-then-keep), or the refusal symbol.
    #[allow(clippy::too_many_arguments)] // The core's arguments, in order.
    pub fn start(
        ruby: &Ruby,
        this: Obj<Self>,
        partition: Value,
        source: Value,
        target: Value,
        base: Value,
        source_token: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (partition, source) = (integer(partition, "partition")?, integer(source, "source")?);
        let (target, base) = (integer(target, "target")?, integer(base, "base")?);
        let held = values::token(ruby, source_token)?;
        let now = duration(now, "now")?;
        let class = classes(ruby)?.ticket.get(ruby);
        let start = |core: &mut Core| core.start(partition, source, target, base, held, now);
        keep(ruby, this, &this.core, start, |started| match started {
            Ok(ticket) => class.funcall("new", (ticket.partition(), ticket.generation())),
            Err(refusal) => Ok(refused(ruby, refusal)),
        })
    }

    /// Every acknowledgement. Frozen is checked and everything that can raise
    /// happens before the core is called; the answer is built while the
    /// decision is held on a copy, kept only once it exists (clone-then-keep).
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
        ticket: Value,
        replica: Value,
        until: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, replica, until, now) = report(ruby, (ticket, replica, until, now), "until")?;
        Self::ack(ruby, this, |core| {
            core.quiesced(ticket, replica, until, now)
        })
    }

    pub fn drained(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        offset: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, replica, offset, now) =
            report(ruby, (ticket, replica, offset, now), "offset")?;
        Self::ack(ruby, this, |core| {
            core.drained(ticket, replica, offset, now)
        })
    }

    pub fn committed(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        offset: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, replica, offset, now) =
            report(ruby, (ticket, replica, offset, now), "offset")?;
        Self::ack(ruby, this, |core| {
            core.committed(ticket, replica, offset, now)
        })
    }

    pub fn captured(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        checkpoint: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let fields = (ticket, replica, checkpoint, now);
        let (ticket, replica, checkpoint, now) = report(ruby, fields, "checkpoint")?;
        Self::ack(ruby, this, |core| {
            core.captured(ticket, replica, checkpoint, now)
        })
    }

    pub fn installed(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        checkpoint: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let fields = (ticket, replica, checkpoint, now);
        let (ticket, replica, checkpoint, now) = report(ruby, fields, "checkpoint")?;
        Self::ack(ruby, this, |core| {
            core.installed(ticket, replica, checkpoint, now)
        })
    }

    pub fn fenced(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        token: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, token) = (values::ticket(ruby, ticket)?, values::token(ruby, token)?);
        let now = duration(now, "now")?;
        Self::ack(ruby, this, |core| core.fenced(ticket, token, now))
    }

    pub fn activated(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        token: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, replica) = (values::ticket(ruby, ticket)?, integer(replica, "replica")?);
        let (token, now) = (values::token(ruby, token)?, duration(now, "now")?);
        Self::ack(ruby, this, |core| {
            core.activated(ticket, replica, token, now)
        })
    }

    pub fn abort(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        token: Option<Value>,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let ticket = values::ticket(ruby, ticket)?;
        let token = token.map(|token| values::token(ruby, token)).transpose()?;
        let now = duration(now, "now")?;
        Self::ack(ruby, this, |core| core.abort(ticket, token, now))
    }

    pub fn resumed(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, replica, now) = settle(ruby, ticket, replica, now)?;
        Self::ack(ruby, this, |core| core.resumed(ticket, replica, now))
    }

    pub fn discarded(
        ruby: &Ruby,
        this: Obj<Self>,
        ticket: Value,
        replica: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (ticket, replica, now) = settle(ruby, ticket, replica, now)?;
        Self::ack(ruby, this, |core| core.discarded(ticket, replica, now))
    }

    pub fn status(ruby: &Ruby, this: &Self, partition: Value) -> Result<Option<Value>, Error> {
        let partition = integer(partition, "partition")?;
        let Some(status) = this.core.borrow().status(partition) else {
            return Ok(None);
        };
        let head = (
            status.ticket().partition(),
            status.ticket().generation(),
            status.source(),
            status.target(),
            names::Phases::symbol(ruby, status.phase()),
        );
        let tail = (
            status.base(),
            status.until(),
            status.checkpoint(),
            status.token().map(FencingToken::raw),
            nanoseconds(ruby, status.deadline()),
        );
        Ok(Some((head, tail).into_value_with(ruby)))
    }

    pub fn orders(
        ruby: &Ruby,
        this: &Self,
        from: Value,
        limit: Value,
    ) -> Result<(RArray, usize), Error> {
        let (from, limit) = (integer(from, "from")?, integer(limit, "limit")?);
        let core = this.core.borrow();
        let mut pass = core.orders(from);
        let list = ruby.ary_new();
        for order in pass.by_ref().take(limit) {
            let (to, action, value) = names::order(ruby, &order);
            let ticket = order.ticket();
            list.push((ticket.partition(), ticket.generation(), to, action, value))?;
        }
        Ok((list, pass.position()))
    }

    pub fn outstanding(
        ruby: &Ruby,
        this: &Self,
        ticket: Value,
        from: Value,
        limit: Value,
    ) -> Result<Value, Error> {
        let (ticket, from) = (values::ticket(ruby, ticket)?, integer(from, "from")?);
        let limit: usize = integer(limit, "limit")?;
        let core = this.core.borrow();
        Ok(match core.outstanding(ticket, from) {
            Ok(missing) => {
                let list = ruby.ary_new();
                for offset in missing.take(limit) {
                    list.push(offset)?;
                }
                list.as_value()
            }
            Err(refusal) => refused(ruby, refusal),
        })
    }

    pub fn overdue(
        ruby: &Ruby,
        this: &Self,
        now: Value,
        from: Value,
        limit: Value,
    ) -> Result<(RArray, usize), Error> {
        let (now, from) = (duration(now, "now")?, integer(from, "from")?);
        let limit = integer(limit, "limit")?;
        let core = this.core.borrow();
        let mut pass = core.overdue(now, from);
        let list = ruby.ary_new();
        for ticket in pass.by_ref().take(limit) {
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
