//! A deliberately naive, allocating restatement of the handoff rules.
use std::{collections::BTreeSet, vec::Vec};

use crate::{Acked, Action, Duration, Limits, Order, Phase, Recipient, Refusal, Status, Ticket};

use periscope_machines::{scope_of, sequence_of};

use super::token;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wait {
    Quiesce,
    Drain,
    Capture,
    Install,
    Fence,
    Activate,
    Undo,
}

#[derive(Clone, Debug)]
pub struct Move {
    pub ticket: Ticket,
    pub source: u64,
    pub target: u64,
    pub base: u64,
    pub held: u64,
    pub wait: Wait,
    pub until: Option<u64>,
    pub acked: BTreeSet<u64>,
    pub checkpoint: Option<u64>,
    pub granted: Option<u64>,
    pub installed: bool,
    pub undo_token: Option<u64>,
    pub owes_resume: bool,
    pub owes_discard: bool,
    pub deadline: Duration,
}

impl Move {
    fn prefix(&self) -> u64 {
        let mut end = self.base;
        while self.acked.contains(&end) {
            end += 1;
        }
        end
    }

    pub fn phase(&self) -> Phase {
        match self.wait {
            Wait::Quiesce => Phase::Quiesce,
            Wait::Drain => Phase::Drain,
            Wait::Capture | Wait::Install => Phase::Checkpoint,
            Wait::Fence | Wait::Activate => Phase::Activate,
            Wait::Undo => Phase::Rollback,
        }
    }

    pub fn missing(&self, from: u64) -> Vec<u64> {
        let until = self.until.unwrap_or(self.base);
        let all = (self.base.max(from)..until).filter(|o| !self.acked.contains(o));
        all.take(200).collect()
    }
}

pub struct Model {
    pub moves: Vec<Move>,
    pub scope: u16,
    /// Sequence number of the last generation issued.
    pub issued: u64,
    pub seen: Duration,
    pub limits: Limits,
    pub capacity: usize,
    pub window: u64,
}

type Outcome = Result<Acked, Refusal>;

/// What a rule answers after an honoured acknowledgement.
enum End {
    /// Recorded; the move goes on.
    No,
    /// Recorded; the move ended in this phase.
    Yes(Phase),
    /// An idempotent repeat: the rule touched nothing.
    Repeat,
}

impl Model {
    pub fn start(
        &mut self,
        p: u64,
        s: u64,
        t: u64,
        base: u64,
        held: u64,
        now: Duration,
    ) -> Result<Ticket, Refusal> {
        let busy = |r: u64| {
            let on = self.moves.iter().filter(|m| m.source == r || m.target == r);
            on.count() >= self.limits.per_replica
        };
        let refusal = if now < self.seen {
            Some(Refusal::ClockRegressed)
        } else if s == t {
            Some(Refusal::SameReplica)
        } else if self.moves.iter().any(|m| m.ticket.partition == p) {
            Some(Refusal::Busy)
        } else if self.moves.len() == self.capacity {
            Some(Refusal::Full)
        } else if busy(s) || busy(t) {
            Some(Refusal::ReplicaBusy)
        } else if self.issued == periscope_machines::SEQUENCE_MAX {
            Some(Refusal::Exhausted)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        self.issued += 1;
        self.seen = now;
        let ticket = Ticket {
            partition: p,
            generation: periscope_machines::scoped(self.scope, self.issued),
        };
        self.moves.push(Move {
            ticket,
            source: s,
            target: t,
            base,
            held,
            wait: Wait::Quiesce,
            until: None,
            acked: BTreeSet::new(),
            checkpoint: None,
            granted: None,
            installed: false,
            undo_token: None,
            owes_resume: false,
            owes_discard: false,
            deadline: now.saturating_add(self.limits.phase_budget),
        });
        Ok(ticket)
    }

    pub fn lookup(&self, ticket: Ticket) -> Result<usize, Refusal> {
        match self.moves.iter().position(|m| m.ticket == ticket) {
            Some(index) => Ok(index),
            None if ticket.scope() != self.scope => Err(Refusal::Unknown),
            None if (1..=self.issued).contains(&sequence_of(ticket.generation)) => {
                Err(Refusal::Stale)
            }
            None => Err(Refusal::Unknown),
        }
    }

    /// Run `rule` on the named move; it answers "ended?" after mutating.
    fn on(
        &mut self,
        ticket: Ticket,
        now: Duration,
        rule: impl FnOnce(&mut Move, u64) -> Result<End, Refusal>,
    ) -> Outcome {
        if now < self.seen {
            return Err(Refusal::ClockRegressed);
        }
        let index = self.lookup(ticket)?;
        let before = self.moves[index].wait;
        match rule(&mut self.moves[index], self.window)? {
            End::Repeat => return Ok(Acked::Duplicate(self.moves[index].phase())),
            End::Yes(phase) => {
                // The end of a transfer holds whatever the time: no clock.
                self.moves.remove(index);
                return Ok(Acked::Recorded(phase));
            }
            End::No => {}
        }
        let entry = &mut self.moves[index];
        // Only a new wait depends on `now`: its deadline counts from it.
        if entry.wait != before {
            entry.deadline = now.saturating_add(self.limits.phase_budget);
            self.seen = now;
        }
        Ok(Acked::Recorded(entry.phase()))
    }

    pub fn quiesced(&mut self, ticket: Ticket, replica: u64, until: u64, now: Duration) -> Outcome {
        self.on(ticket, now, |m, _| {
            if let Some(recorded) = m.until {
                return replay(replica == m.source, recorded == until);
            }
            check(m.wait == Wait::Quiesce, Refusal::OutOfPhase)?;
            check(replica == m.source, Refusal::WrongReplica)?;
            check(until >= m.base, Refusal::BehindBase)?;
            m.until = Some(until);
            m.wait = if until == m.base {
                Wait::Capture
            } else {
                Wait::Drain
            };
            Ok(End::No)
        })
    }

    pub fn drained(&mut self, ticket: Ticket, replica: u64, offset: u64, now: Duration) -> Outcome {
        self.on(ticket, now, |m, window| {
            check(m.until.is_some(), Refusal::OutOfPhase)?;
            check(replica == m.source, Refusal::WrongReplica)?;
            check(Some(offset) < m.until, Refusal::BeyondDrain)?;
            if offset < m.base || m.acked.contains(&offset) {
                return Ok(End::Repeat);
            }
            check(m.wait == Wait::Drain, Refusal::OutOfPhase)?;
            check(offset - m.base < window, Refusal::OutOfWindow)?;
            m.acked.insert(offset);
            if Some(m.prefix()) == m.until {
                m.wait = Wait::Capture;
            }
            Ok(End::No)
        })
    }

    pub fn committed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        offset: u64,
        now: Duration,
    ) -> Outcome {
        self.on(ticket, now, |m, _| {
            check(m.until.is_some(), Refusal::OutOfPhase)?;
            check(replica == m.source, Refusal::WrongReplica)?;
            if offset <= m.base {
                return Ok(End::Repeat);
            }
            check(m.wait == Wait::Drain, Refusal::OutOfPhase)?;
            check(offset <= m.prefix(), Refusal::Unacked)?;
            m.base = offset;
            m.acked.retain(|o| *o >= offset);
            Ok(End::No)
        })
    }

    pub fn captured(
        &mut self,
        ticket: Ticket,
        replica: u64,
        checkpoint: u64,
        now: Duration,
    ) -> Outcome {
        self.on(ticket, now, |m, _| {
            if let Some(recorded) = m.checkpoint {
                return replay(replica == m.source, recorded == checkpoint);
            }
            check(m.wait == Wait::Capture, Refusal::OutOfPhase)?;
            check(replica == m.source, Refusal::WrongReplica)?;
            m.checkpoint = Some(checkpoint);
            m.wait = Wait::Install;
            Ok(End::No)
        })
    }

    pub fn installed(
        &mut self,
        ticket: Ticket,
        replica: u64,
        checkpoint: u64,
        now: Duration,
    ) -> Outcome {
        self.on(ticket, now, |m, _| {
            if m.installed {
                return replay(replica == m.target, m.checkpoint == Some(checkpoint));
            }
            check(m.wait == Wait::Install, Refusal::OutOfPhase)?;
            check(replica == m.target, Refusal::WrongReplica)?;
            check(m.checkpoint == Some(checkpoint), Refusal::Mismatch)?;
            m.installed = true;
            m.wait = Wait::Fence;
            Ok(End::No)
        })
    }

    pub fn fenced(&mut self, ticket: Ticket, granted: u64, now: Duration) -> Outcome {
        self.on(ticket, now, |m, _| {
            homed(granted, m.held)?;
            if let Some(recorded) = m.granted {
                return replay(true, recorded == granted);
            }
            check(m.wait == Wait::Fence, Refusal::OutOfPhase)?;
            check(granted > m.held, Refusal::StaleToken)?;
            m.granted = Some(granted);
            m.wait = Wait::Activate;
            Ok(End::No)
        })
    }

    pub fn activated(
        &mut self,
        ticket: Ticket,
        replica: u64,
        granted: u64,
        now: Duration,
    ) -> Outcome {
        self.on(ticket, now, |m, _| {
            homed(granted, m.held)?;
            check(m.wait == Wait::Activate, Refusal::OutOfPhase)?;
            check(replica == m.target, Refusal::WrongReplica)?;
            check(m.granted == Some(granted), Refusal::Mismatch)?;
            Ok(End::Yes(Phase::Complete))
        })
    }

    pub fn abort(&mut self, ticket: Ticket, fence: Option<u64>, now: Duration) -> Outcome {
        self.on(ticket, now, |m, _| {
            fence.map_or(Ok(()), |fence| homed(fence, m.held))?;
            if m.wait == Wait::Undo {
                return replay(true, m.undo_token == fence);
            }
            let known = m.granted.unwrap_or(m.held);
            check(fence.is_none_or(|fence| fence > known), Refusal::StaleToken)?;
            check(
                fence.is_some() || m.wait != Wait::Activate,
                Refusal::FenceRequired,
            )?;
            m.owes_resume = true;
            m.owes_discard = [Wait::Install, Wait::Fence, Wait::Activate].contains(&m.wait);
            m.undo_token = fence;
            m.wait = Wait::Undo;
            Ok(End::No)
        })
    }

    pub fn undone(
        &mut self,
        ticket: Ticket,
        replica: u64,
        by_source: bool,
        now: Duration,
    ) -> Outcome {
        self.on(ticket, now, |m, _| {
            check(m.wait == Wait::Undo, Refusal::OutOfPhase)?;
            let holder = if by_source { m.source } else { m.target };
            check(replica == holder, Refusal::WrongReplica)?;
            let owed = if by_source {
                &mut m.owes_resume
            } else {
                &mut m.owes_discard
            };
            if !*owed {
                return Ok(End::Repeat);
            }
            *owed = false;
            let done = !m.owes_resume && !m.owes_discard;
            Ok(if done {
                End::Yes(Phase::Aborted)
            } else {
                End::No
            })
        })
    }

    pub fn status(&self, partition: u64) -> Option<Status> {
        let m = self
            .moves
            .iter()
            .find(|m| m.ticket.partition == partition)?;
        Some(Status {
            ticket: m.ticket,
            source: m.source,
            target: m.target,
            phase: m.phase(),
            base: m.base,
            until: m.until,
            checkpoint: m.checkpoint,
            token: m.granted.map(token),
            deadline: m.deadline,
        })
    }

    /// Every outstanding order, in no particular order.
    pub fn orders(&self) -> Vec<Order> {
        let mut out = Vec::new();
        for m in &self.moves {
            let (source, target) = (Recipient::Replica(m.source), Recipient::Replica(m.target));
            let mut push = |to, action| {
                out.push(Order {
                    ticket: m.ticket,
                    to,
                    action,
                });
            };
            let (until, checkpoint) = (m.until.unwrap_or(0), m.checkpoint.unwrap_or(0));
            let above = token(m.held);
            match m.wait {
                Wait::Quiesce => push(source, Action::Quiesce),
                Wait::Drain => push(source, Action::Drain { until }),
                Wait::Capture => push(source, Action::Capture { at: until }),
                Wait::Install => push(target, Action::Install { checkpoint }),
                Wait::Fence => push(Recipient::Authority, Action::Fence { above }),
                Wait::Activate => {
                    let token = token(m.granted.unwrap());
                    push(target, Action::Activate { token });
                }
                Wait::Undo => {
                    if m.owes_resume {
                        push(
                            source,
                            Action::Resume {
                                token: m.undo_token.map(token),
                            },
                        );
                    }
                    if m.owes_discard {
                        push(target, Action::Discard);
                    }
                }
            }
        }
        out
    }
}

/// A recorded acknowledgement arrives again: a repeat only from the same
/// holder with the same payload.
fn replay(holder: bool, same: bool) -> Result<End, Refusal> {
    check(holder, Refusal::WrongReplica)?;
    check(same, Refusal::Mismatch)?;
    Ok(End::Repeat)
}

fn check(holds: bool, refusal: Refusal) -> Result<(), Refusal> {
    if holds { Ok(()) } else { Err(refusal) }
}

/// A raw token means something to a transfer only inside the scope of the
/// token its source `held`: numbers of two scopes are never ranked against
/// each other, nor compared with what the transfer recorded.
fn homed(token: u64, held: u64) -> Result<(), Refusal> {
    check(scope_of(token) == scope_of(held), Refusal::Unknown)
}
