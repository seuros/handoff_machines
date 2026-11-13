use core::time::Duration;

use lease_machines::FencingToken;
use offset_machines::{Ack, Commit, Tracker};

use crate::{Action, Phase, Recipient, Refusal, Status, Ticket};

/// The coordinator's name for a refusal of the transfer's drain tracker.
const fn tracked(refusal: offset_machines::Refusal) -> Refusal {
    match refusal {
        offset_machines::Refusal::Stale => Refusal::Stale,
        offset_machines::Refusal::OutOfWindow => Refusal::OutOfWindow,
        offset_machines::Refusal::Unacked => Refusal::Unacked,
        offset_machines::Refusal::Exhausted => Refusal::Exhausted,
    }
}

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

/// Result of an honoured acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Idempotent repeat; nothing was recorded and nothing changed.
    Repeat,
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
    installed: bool,
    tracker: Tracker<WORDS>,
}

/// Answer to an acknowledgement the transfer already recorded: an idempotent
/// repeat when the same holder states the same payload, a refusal otherwise.
const fn replay(holder: bool, same: bool) -> Result<Next, Refusal> {
    if !holder {
        Err(Refusal::WrongReplica)
    } else if same {
        Ok(Next::Repeat)
    } else {
        Err(Refusal::Mismatch)
    }
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
        tracker.assign(ticket.generation, base).ok()?;
        Some(Self {
            ticket,
            source,
            target,
            deadline,
            source_token,
            step: Step::Quiesce,
            until: None,
            checkpoint: None,
            token: None,
            installed: false,
            tracker,
        })
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
        let (first, second) = match self.step {
            Step::Quiesce => (Some((source, Action::Quiesce)), None),
            Step::Drain => match self.until {
                Some(until) => (Some((source, Action::Drain { until })), None),
                None => (None, None),
            },
            Step::Capture => match self.until {
                Some(at) => (Some((source, Action::Capture { at })), None),
                None => (None, None),
            },
            Step::Install => match self.checkpoint {
                Some(checkpoint) => (Some((target, Action::Install { checkpoint })), None),
                None => (None, None),
            },
            Step::Fence => {
                let above = self.source_token;
                (Some((Recipient::Authority, Action::Fence { above })), None)
            }
            Step::Activate => match self.token {
                Some(token) => (Some((target, Action::Activate { token })), None),
                None => (None, None),
            },
            Step::Rollback {
                resume,
                discard,
                token,
            } => (
                if resume {
                    Some((source, Action::Resume { token }))
                } else {
                    None
                },
                if discard {
                    Some((target, Action::Discard))
                } else {
                    None
                },
            ),
        };
        match part {
            0 => first,
            1 => second,
            _ => None,
        }
    }

    /// Bounds of the still-unacknowledged drain: tracker and quiesce point.
    pub fn draining(&self) -> Option<(&Tracker<WORDS>, u64)> {
        let until = self.until?;
        (self.step == Step::Drain).then_some((&self.tracker, until))
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
        if let Some(recorded) = self.until {
            return replay(replica == self.source, recorded == until);
        }
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
        let until = self.until.ok_or(Refusal::OutOfPhase)?;
        if replica != self.source {
            return Err(Refusal::WrongReplica);
        }
        if offset >= until {
            return Err(Refusal::BeyondDrain);
        }
        if self.step != Step::Drain {
            // The drain is over or was given up: only what it recorded repeats.
            return if self.tracker.is_acked(offset) {
                Ok(Next::Repeat)
            } else {
                Err(Refusal::OutOfPhase)
            };
        }
        let ack = self.tracker.ack(self.ticket.generation, offset);
        match ack.map_err(tracked)? {
            Ack::Duplicate => return Ok(Next::Repeat),
            Ack::Recorded => {}
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
        if self.until.is_none() {
            return Err(Refusal::OutOfPhase);
        }
        if replica != self.source {
            return Err(Refusal::WrongReplica);
        }
        if self.step != Step::Drain {
            return if offset <= self.tracker.base() {
                Ok(Next::Repeat)
            } else {
                Err(Refusal::OutOfPhase)
            };
        }
        let commit = self.tracker.mark_committed(self.ticket.generation, offset);
        Ok(match commit.map_err(tracked)? {
            Commit::Duplicate => Next::Repeat,
            Commit::Advanced => Next::Same,
        })
    }

    pub fn captured(&mut self, replica: u64, checkpoint: u64) -> Result<Next, Refusal> {
        if let Some(recorded) = self.checkpoint {
            return replay(replica == self.source, recorded == checkpoint);
        }
        self.expect(Step::Capture, replica, self.source)?;
        self.checkpoint = Some(checkpoint);
        self.step = Step::Install;
        Ok(Next::Moved)
    }

    pub fn installed(&mut self, replica: u64, checkpoint: u64) -> Result<Next, Refusal> {
        if self.installed {
            let same = self.checkpoint == Some(checkpoint);
            return replay(replica == self.target, same);
        }
        self.expect(Step::Install, replica, self.target)?;
        if self.checkpoint != Some(checkpoint) {
            return Err(Refusal::Mismatch);
        }
        self.installed = true;
        self.step = Step::Fence;
        Ok(Next::Moved)
    }

    pub fn fenced(&mut self, token: FencingToken) -> Result<Next, Refusal> {
        if let Some(recorded) = self.token {
            return replay(true, recorded == token);
        }
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
        let discard = match self.step {
            Step::Rollback {
                token: recorded, ..
            } => return replay(true, recorded == token),
            Step::Quiesce | Step::Drain | Step::Capture => false,
            Step::Install | Step::Fence | Step::Activate => true,
        };
        let floor = self.token.unwrap_or(self.source_token);
        if token.is_some_and(|token| token <= floor) {
            return Err(Refusal::StaleToken);
        }
        if token.is_none() && self.step == Step::Activate {
            return Err(Refusal::FenceRequired);
        }
        self.step = Step::Rollback {
            resume: true,
            discard,
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
            return Ok(Next::Repeat);
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
