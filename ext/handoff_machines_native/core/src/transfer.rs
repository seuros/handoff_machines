use core::time::Duration;

use lease_machines::FencingToken;
use offset_machines::{Ack, Commit, Tracker};

use crate::{Action, Phase, Recipient, Refusal, Status, Ticket};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Quiesce,
    Drain,
    Capture,
    Install,
    Fence,
    Activate,
    Rollback {
        resume: bool,
        discard: bool,
        token: Option<FencingToken>,
    },
}

/// Result of an accepted acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Recorded; the transfer waits for the same thing.
    Same,
    /// The transfer waits for something new.
    Moved,
    /// The transfer ended; free its slot.
    Done(Phase),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer<const WORDS: usize> {
    pub ticket: Ticket,
    pub source: u64,
    pub target: u64,
    pub deadline: Duration,
    source_token: FencingToken,
    step: Step,
    until: Option<u64>,
    checkpoint: Option<u64>,
    token: Option<FencingToken>,
    tracker: Tracker<WORDS>,
}

impl<const WORDS: usize> Transfer<WORDS> {
    pub fn new(
        ticket: Ticket,
        source: u64,
        target: u64,
        base: u64,
        source_token: FencingToken,
        deadline: Duration,
    ) -> Option<Self> {
        let mut tracker = Tracker::new();
        (tracker.assign(ticket.generation, base) == offset_machines::Assign::Assigned).then_some(
            Self {
                ticket,
                source,
                target,
                deadline,
                source_token,
                step: Step::Quiesce,
                until: None,
                checkpoint: None,
                token: None,
                tracker,
            },
        )
    }

    pub const fn involves(&self, replica: u64) -> bool {
        self.source == replica || self.target == replica
    }

    pub const fn phase(&self) -> Phase {
        match self.step {
            Step::Quiesce => Phase::Quiesce,
            Step::Drain => Phase::Drain,
            Step::Capture | Step::Install => Phase::Checkpoint,
            Step::Fence | Step::Activate => Phase::Activate,
            Step::Rollback { .. } => Phase::Rollback,
        }
    }

    pub const fn status(&self) -> Status {
        Status {
            ticket: self.ticket,
            source: self.source,
            target: self.target,
            phase: self.phase(),
            base: self.tracker.base(),
            until: self.until,
            checkpoint: self.checkpoint,
            token: self.token,
            deadline: self.deadline,
        }
    }

    /// The order restated at `part` (0 or 1) of this transfer's slot.
    pub const fn order(&self, part: usize) -> Option<(Recipient, Action)> {
        let source = Recipient::Replica(self.source);
        let target = Recipient::Replica(self.target);
        match (self.step, part, self.until, self.checkpoint, self.token) {
            (Step::Quiesce, 0, ..) => Some((source, Action::Quiesce)),
            (Step::Drain, 0, Some(until), ..) => Some((source, Action::Drain { until })),
            (Step::Capture, 0, Some(at), ..) => Some((source, Action::Capture { at })),
            (Step::Install, 0, _, Some(checkpoint), _) => {
                Some((target, Action::Install { checkpoint }))
            }
            (Step::Fence, 0, ..) => Some((
                Recipient::Authority,
                Action::Fence {
                    above: self.source_token,
                },
            )),
            (Step::Activate, 0, _, _, Some(token)) => Some((target, Action::Activate { token })),
            (
                Step::Rollback {
                    resume: true,
                    token,
                    ..
                },
                0,
                ..,
            ) => Some((source, Action::Resume { token })),
            (Step::Rollback { discard: true, .. }, 1, ..) => Some((target, Action::Discard)),
            _ => None,
        }
    }

    /// Bounds of the still-unacknowledged drain: tracker and quiesce point.
    pub const fn draining(&self) -> Option<(&Tracker<WORDS>, u64)> {
        match (self.step, self.until) {
            (Step::Drain, Some(until)) => Some((&self.tracker, until)),
            _ => None,
        }
    }

    fn expect(&self, step: Step, replica: u64, holder: u64) -> Result<(), Refusal> {
        if self.step != step {
            Err(Refusal::OutOfPhase)
        } else if replica != holder {
            Err(Refusal::WrongReplica)
        } else {
            Ok(())
        }
    }

    pub fn quiesced(&mut self, replica: u64, until: u64) -> Result<Next, Refusal> {
        self.expect(Step::Quiesce, replica, self.source)?;
        if until < self.tracker.base() {
            return Err(Refusal::BehindBase);
        }
        self.until = Some(until);
        self.step = if until == self.tracker.base() {
            Step::Capture
        } else {
            Step::Drain
        };
        Ok(Next::Moved)
    }

    pub fn drained(&mut self, replica: u64, offset: u64) -> Result<Next, Refusal> {
        self.expect(Step::Drain, replica, self.source)?;
        let until = self.until.ok_or(Refusal::OutOfPhase)?;
        if offset >= until {
            return Err(Refusal::BeyondDrain);
        }
        match self.tracker.ack(self.ticket.generation, offset) {
            Ack::Ok => {}
            Ack::Duplicate => return Err(Refusal::Duplicate),
            Ack::Stale => return Err(Refusal::Stale),
            _ => return Err(Refusal::OutOfWindow),
        }
        let reached = self
            .tracker
            .commit_candidate()
            .unwrap_or_else(|| self.tracker.base());
        if reached < until {
            return Ok(Next::Same);
        }
        self.step = Step::Capture;
        Ok(Next::Moved)
    }

    pub fn committed(&mut self, replica: u64, offset: u64) -> Result<Next, Refusal> {
        self.expect(Step::Drain, replica, self.source)?;
        match self.tracker.mark_committed(self.ticket.generation, offset) {
            Commit::Advanced => Ok(Next::Same),
            Commit::Duplicate => Err(Refusal::Duplicate),
            Commit::Stale => Err(Refusal::Stale),
            _ => Err(Refusal::Unacked),
        }
    }

    pub fn captured(&mut self, replica: u64, checkpoint: u64) -> Result<Next, Refusal> {
        self.expect(Step::Capture, replica, self.source)?;
        self.checkpoint = Some(checkpoint);
        self.step = Step::Install;
        Ok(Next::Moved)
    }

    pub fn installed(&mut self, replica: u64, checkpoint: u64) -> Result<Next, Refusal> {
        self.expect(Step::Install, replica, self.target)?;
        if self.checkpoint != Some(checkpoint) {
            return Err(Refusal::Mismatch);
        }
        self.step = Step::Fence;
        Ok(Next::Moved)
    }

    pub fn fenced(&mut self, token: FencingToken) -> Result<Next, Refusal> {
        if self.step != Step::Fence {
            return Err(Refusal::OutOfPhase);
        }
        if token <= self.source_token {
            return Err(Refusal::StaleToken);
        }
        self.token = Some(token);
        self.step = Step::Activate;
        Ok(Next::Moved)
    }

    pub fn activated(&self, replica: u64, token: FencingToken) -> Result<Next, Refusal> {
        self.expect(Step::Activate, replica, self.target)?;
        if self.token != Some(token) {
            return Err(Refusal::Mismatch);
        }
        Ok(Next::Done(Phase::Complete))
    }

    pub fn abort(&mut self, token: Option<FencingToken>) -> Result<Next, Refusal> {
        if matches!(self.step, Step::Rollback { .. }) {
            return Err(Refusal::OutOfPhase);
        }
        let floor = self.token.unwrap_or(self.source_token);
        if token.is_some_and(|token| token <= floor) {
            return Err(Refusal::StaleToken);
        }
        if token.is_none() && self.step == Step::Activate {
            return Err(Refusal::FenceRequired);
        }
        self.step = Step::Rollback {
            resume: true,
            discard: matches!(self.step, Step::Install | Step::Fence | Step::Activate),
            token,
        };
        Ok(Next::Moved)
    }

    pub const fn resumed(&mut self, replica: u64) -> Result<Next, Refusal> {
        self.undone(replica, self.source, true)
    }

    pub const fn discarded(&mut self, replica: u64) -> Result<Next, Refusal> {
        self.undone(replica, self.target, false)
    }

    const fn undone(&mut self, replica: u64, holder: u64, source: bool) -> Result<Next, Refusal> {
        let Step::Rollback {
            resume,
            discard,
            token,
        } = self.step
        else {
            return Err(Refusal::OutOfPhase);
        };
        if replica != holder {
            return Err(Refusal::WrongReplica);
        }
        let (pending, resume, discard) = if source {
            (resume, false, discard)
        } else {
            (discard, resume, false)
        };
        if !pending {
            return Err(Refusal::Duplicate);
        }
        if !resume && !discard {
            return Ok(Next::Done(Phase::Aborted));
        }
        self.step = Step::Rollback {
            resume,
            discard,
            token,
        };
        Ok(Next::Same)
    }
}
