//! Randomized comparison of the core against the naive model.
use std::{collections::BTreeSet, format, string::String, vec::Vec};

use super::model::{Model, Wait};
use super::{LIMITS, token};
use crate::{Duration, Handoff, Order, Phase, Refusal, Ticket};

const N: usize = 4;
const WORDS: usize = 1;

struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn sorted(orders: impl IntoIterator<Item = Order>) -> Vec<String> {
    let mut lines: Vec<String> = orders.into_iter().map(|o| format!("{o:?}")).collect();
    lines.sort();
    lines
}

struct Run {
    core: Handoff<N, WORDS>,
    model: Model,
    draws: Draws,
    now: Duration,
    tickets: Vec<Ticket>,
    seen: BTreeSet<String>,
}

impl Run {
    fn new(seed: u64) -> Self {
        Self {
            core: Handoff::new(LIMITS),
            model: Model {
                moves: Vec::new(),
                issued: 0,
                seen: Duration::ZERO,
                limits: LIMITS,
                capacity: N,
                window: 64,
            },
            draws: Draws(seed),
            now: Duration::ZERO,
            tickets: Vec::new(),
            seen: BTreeSet::new(),
        }
    }

    fn clock(&mut self) -> Duration {
        if self.draws.chance(4) {
            return self
                .now
                .saturating_sub(Duration::from_secs(1 + self.draws.below(3)));
        }
        if self.draws.chance(40) {
            self.now += Duration::from_secs(self.draws.below(4));
        }
        self.now
    }

    fn ticket(&mut self) -> Ticket {
        let roll = self.draws.below(100);
        if roll < 6 || self.tickets.is_empty() {
            return Ticket {
                partition: self.draws.below(6),
                generation: self.draws.below(self.model.issued + 3),
            };
        }
        if roll < 14 || self.model.moves.is_empty() {
            let pick = self.draws.below(self.tickets.len() as u64);
            return self.tickets[usize::try_from(pick).unwrap()];
        }
        let pick = self.draws.below(self.model.moves.len() as u64);
        self.model.moves[usize::try_from(pick).unwrap()].ticket
    }

    fn step(&mut self) {
        let now = self.clock();
        let before = self.core.clone();
        if self.draws.chance(12) {
            let (p, s, t) = (
                self.draws.below(6),
                self.draws.below(5),
                self.draws.below(5),
            );
            let base = if self.draws.chance(10) {
                u64::MAX - self.draws.below(40)
            } else {
                self.draws.below(10)
            };
            let held = 1 + self.draws.below(5);
            let got = self.core.start(p, s, t, base, token(held), now);
            assert_eq!(got, self.model.start(p, s, t, base, held, now));
            self.record(&got.map(|_| Phase::Quiesce), &before);
            self.tickets.extend(got.ok());
        } else {
            let ticket = self.ticket();
            let got = self.ack(ticket, now);
            self.record(&got, &before);
        }
        self.compare();
    }

    /// One acknowledgement: usually the one the move waits for, sometimes not.
    #[allow(clippy::too_many_lines)] // One arm per acknowledgement kind reads best in one place.
    fn ack(&mut self, ticket: Ticket, now: Duration) -> Result<Phase, Refusal> {
        let entry = self
            .model
            .lookup(ticket)
            .ok()
            .map(|i| self.model.moves[i].clone());
        let (source, target, base) = entry
            .as_ref()
            .map_or((0, 1, 0), |m| (m.source, m.target, m.base));
        let honest = !self.draws.chance(12);
        let by = |d: &mut Draws, holder: u64| if d.chance(92) { holder } else { d.below(5) };
        let wait = entry.as_ref().map(|m| m.wait);
        let choice = match wait {
            Some(Wait::Quiesce) if honest => 0,
            Some(Wait::Drain) if honest => {
                1 + self.draws.below(8).min(1) * u64::from(self.draws.chance(15))
            }
            Some(Wait::Capture) if honest => 3,
            Some(Wait::Install) if honest => 4,
            Some(Wait::Fence) if honest => 5,
            Some(Wait::Activate) if honest => 6,
            Some(Wait::Undo) if honest => 8 + self.draws.below(2),
            _ => self.draws.below(10),
        };
        let choice = if self.draws.chance(5) { 7 } else { choice };
        let (core, model, d) = (&mut self.core, &mut self.model, &mut self.draws);
        let near = |d: &mut Draws, value: Option<u64>| match value {
            Some(value) if d.chance(92) => value,
            _ => d.below(9),
        };
        let (got, expected) = match choice {
            0 => {
                let until = base
                    .saturating_add(d.below(90))
                    .saturating_sub(d.below(2) * d.below(2));
                let who = by(d, source);
                (
                    core.quiesced(ticket, who, until, now),
                    model.quiesced(ticket, who, until, now),
                )
            }
            1 => {
                let missing = entry.as_ref().map_or_default(|m| m.missing(0));
                let offset = match missing.len() as u64 {
                    count if count > 0 && d.chance(90) => {
                        let reach = if d.chance(80) { count.min(3) } else { count };
                        missing[usize::try_from(d.below(reach)).unwrap()]
                    }
                    _ => base.saturating_add(d.below(100)).saturating_sub(d.below(3)),
                };
                let who = by(d, source);
                (
                    core.drained(ticket, who, offset, now),
                    model.drained(ticket, who, offset, now),
                )
            }
            2 => {
                let offset = base.saturating_add(d.below(6));
                let who = by(d, source);
                (
                    core.committed(ticket, who, offset, now),
                    model.committed(ticket, who, offset, now),
                )
            }
            3 => {
                let (who, id) = (by(d, source), d.below(9));
                (
                    core.captured(ticket, who, id, now),
                    model.captured(ticket, who, id, now),
                )
            }
            4 => {
                let (who, id) = (
                    by(d, target),
                    near(d, entry.as_ref().and_then(|m| m.checkpoint)),
                );
                (
                    core.installed(ticket, who, id, now),
                    model.installed(ticket, who, id, now),
                )
            }
            5 => {
                let grant = 1 + d.below(9);
                (
                    core.fenced(ticket, token(grant), now),
                    model.fenced(ticket, grant, now),
                )
            }
            6 => {
                let grant = 1 + near(d, entry.as_ref().and_then(|m| m.granted).map(|g| g - 1));
                let who = by(d, target);
                (
                    core.activated(ticket, who, token(grant), now),
                    model.activated(ticket, who, grant, now),
                )
            }
            7 => {
                let fence = d.chance(50).then(|| 1 + d.below(12));
                (
                    core.abort(ticket, fence.map(token), now),
                    model.abort(ticket, fence, now),
                )
            }
            8 => {
                let who = by(d, source);
                (
                    core.resumed(ticket, who, now),
                    model.undone(ticket, who, true, now),
                )
            }
            _ => {
                let who = by(d, target);
                (
                    core.discarded(ticket, who, now),
                    model.undone(ticket, who, false, now),
                )
            }
        };
        assert_eq!(
            got, expected,
            "choice {choice} on {ticket:?} waiting for {wait:?}"
        );
        got
    }

    fn record<T: core::fmt::Debug>(
        &mut self,
        got: &Result<T, Refusal>,
        before: &Handoff<N, WORDS>,
    ) {
        if got.is_err() {
            assert_eq!(&self.core, before, "a refused call mutated state: {got:?}");
        }
        self.seen.insert(format!("{got:?}"));
    }

    fn compare(&self) {
        assert_eq!(self.core.active(), self.model.moves.len());
        assert_eq!(self.core.generation(), self.model.issued);
        for partition in 0..6 {
            assert_eq!(self.core.status(partition), self.model.status(partition));
        }
        assert_eq!(sorted(self.core.orders(0)), sorted(self.model.orders()));
        let mut late: Vec<Ticket> = self.core.overdue(self.now, 0).collect();
        late.sort_by_key(|t| t.generation);
        let due = self.model.moves.iter().filter(|m| self.now >= m.deadline);
        let mut expected: Vec<Ticket> = due.map(|m| m.ticket).collect();
        expected.sort_by_key(|t| t.generation);
        assert_eq!(late, expected);
        for entry in &self.model.moves {
            let from = (entry.base.saturating_sub(1)).saturating_add(entry.ticket.generation % 4);
            let listed = self.core.outstanding(entry.ticket, from);
            if entry.wait == Wait::Drain {
                let listed: Vec<u64> = listed.unwrap().take(200).collect();
                assert_eq!(listed, entry.missing(from));
            } else {
                assert_eq!(listed.err(), Some(Refusal::OutOfPhase));
            }
        }
    }
}

#[test]
fn core_matches_the_naive_model_under_random_operations() {
    let mut seen = BTreeSet::new();
    for seed in 1..=40_u64 {
        let mut run = Run::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for _ in 0..4_000 {
            run.step();
        }
        seen.append(&mut run.seen);
    }
    let expected = [
        "Ok(Quiesce)",
        "Ok(Drain)",
        "Ok(Checkpoint)",
        "Ok(Activate)",
        "Ok(Rollback)",
        "Ok(Complete)",
        "Ok(Aborted)",
        "Err(Full)",
        "Err(Busy)",
        "Err(ReplicaBusy)",
        "Err(SameReplica)",
        "Err(ClockRegressed)",
        "Err(Stale)",
        "Err(Unknown)",
        "Err(OutOfPhase)",
        "Err(WrongReplica)",
        "Err(BehindBase)",
        "Err(BeyondDrain)",
        "Err(Duplicate)",
        "Err(OutOfWindow)",
        "Err(Unacked)",
        "Err(Mismatch)",
        "Err(StaleToken)",
        "Err(FenceRequired)",
    ];
    for outcome in expected {
        assert!(
            seen.contains(outcome),
            "never exercised {outcome}: {seen:?}"
        );
    }
}
