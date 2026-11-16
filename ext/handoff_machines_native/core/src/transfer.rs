use core::{cmp::Ordering, num::NonZeroU64, time::Duration};

use lease_machines::FencingToken;
use offset_machines::{Ack, Commit, Tracker};

use crate::{Action, Phase, Recipient, Refusal, Status, Ticket};

/// What the transfer waits for next. Each step carries what its order
/// restates, so no step can stand without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Quiesce,
    Drain {
        until: u64,
    },
    Capture {
        at: u64,
    },
    Install {
        checkpoint: u64,
    },
    Fence,
    Activate {
        token: FencingToken,
    },
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

/// Whether `token` may supersede `floor`, the newest token the transfer knows:
/// `StaleToken` unless it exceeds it.
///
/// Both are of the transfer's one token scope: `token` passed
/// [`Transfer::homed`], and `floor` is the source token or the activation
/// token `fenced` admitted. The ranking is the token's own, which never orders
/// tokens across scopes: tokens of two scopes are `Unknown` here too, the
/// answer `homed` already gave, so no raw value is compared without its scope.
const fn supersedes(token: FencingToken, floor: FencingToken) -> Result<(), Refusal> {
    match token.order(floor) {
        Some(Ordering::Greater) => Ok(()),
        Some(Ordering::Less | Ordering::Equal) => Err(Refusal::StaleToken),
        None => Err(Refusal::Unknown),
    }
}

/// The drain tracker's one generation. The tracker belongs to one transfer
/// and never meets another assignment: the ticket is checked before the
/// tracker is reached, so the tracker needs no generation of its own beyond
/// being assigned, and [`Tracker::assigned`] builds it so without a refusal.
const DRAIN: NonZeroU64 = NonZeroU64::MIN;

/// The answer for a drain-tracker refusal the transfer rules out: the tracker
/// is assigned at construction and only ever asked under its own current
/// generation, so `Stale` cannot come back; it is only asked to acknowledge
/// or confirm, so `Exhausted` cannot either, and `ack` never answers
/// `Unacked` nor `mark_committed` `OutOfWindow`. The tracker's one `Refusal`
/// type still names them. Refused as `OutOfPhase`, the most conservative
/// answer; the tracker refused, so nothing changed.
fn untracked() -> Refusal {
    debug_assert!(
        false,
        "the drain tracker is asked only under its current generation"
    );
    Refusal::OutOfPhase
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
    pub const fn new(
        ticket: Ticket,
        source: u64,
        target: u64,
        base: u64,
        source_token: FencingToken,
        deadline: Duration,
    ) -> Self {
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
            installed: false,
            tracker: Tracker::assigned(DRAIN, base),
        }
    }

    pub const fn involves(&self, replica: u64) -> bool {
        self.source == replica || self.target == replica
    }

    pub const fn phase(&self) -> Phase {
        match self.step {
            Step::Quiesce => Phase::Quiesce,
            Step::Drain { .. } => Phase::Drain,
            Step::Capture { .. } | Step::Install { .. } => Phase::Checkpoint,
            Step::Fence | Step::Activate { .. } => Phase::Activate,
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
            Step::Drain { until } => (Some((source, Action::Drain { until })), None),
            Step::Capture { at } => (Some((source, Action::Capture { at })), None),
            Step::Install { checkpoint } => (Some((target, Action::Install { checkpoint })), None),
            Step::Fence => {
                let above = self.source_token;
                (Some((Recipient::Authority, Action::Fence { above })), None)
            }
            Step::Activate { token } => (Some((target, Action::Activate { token })), None),
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
    pub const fn draining(&self) -> Option<(&Tracker<WORDS>, u64)> {
        match self.step {
            Step::Drain { until } => Some((&self.tracker, until)),
            Step::Quiesce
            | Step::Capture { .. }
            | Step::Install { .. }
            | Step::Fence
            | Step::Activate { .. }
            | Step::Rollback { .. } => None,
        }
    }

    const fn draining_now(&self) -> bool {
        self.draining().is_some()
    }

    /// A token of another scope than the source token's names nothing in
    /// this transfer: `Unknown`, answered before the step or any recorded
    /// token is read. Every token the transfer records has passed this, so
    /// the source token's scope is the transfer's one token scope.
    const fn homed(&self, token: FencingToken) -> Result<(), Refusal> {
        if token.scope() == self.source_token.scope() {
            Ok(())
        } else {
            Err(Refusal::Unknown)
        }
    }

    /// `OutOfPhase` unless the transfer is `waiting` for this, then
    /// `WrongReplica` unless `replica` is the `holder`.
    const fn expect(waiting: bool, replica: u64, holder: u64) -> Result<(), Refusal> {
        if !waiting {
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
        Self::expect(matches!(self.step, Step::Quiesce), replica, self.source)?;
        if until < self.tracker.base() {
            return Err(Refusal::BehindBase);
        }
        self.until = Some(until);
        self.step = if until == self.tracker.base() {
            Step::Capture { at: until }
        } else {
            Step::Drain { until }
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
        if !self.draining_now() {
            // The drain is over or was given up: only what it recorded repeats.
            return if self.tracker.is_acked(offset) {
                Ok(Next::Repeat)
            } else {
                Err(Refusal::OutOfPhase)
            };
        }
        match self.tracker.ack(DRAIN.get(), offset) {
            Ok(Ack::Duplicate) => return Ok(Next::Repeat),
            Ok(Ack::Recorded) => {}
            Err(offset_machines::Refusal::OutOfWindow) => return Err(Refusal::OutOfWindow),
            Err(
                offset_machines::Refusal::Stale
                | offset_machines::Refusal::Unacked
                | offset_machines::Refusal::Exhausted,
            ) => return Err(untracked()),
        }
        let reached = self
            .tracker
            .commit_candidate()
            .unwrap_or_else(|| self.tracker.base());
        if reached < until {
            return Ok(Next::Same);
        }
        self.step = Step::Capture { at: until };
        Ok(Next::Moved)
    }

    pub fn committed(&mut self, replica: u64, offset: u64) -> Result<Next, Refusal> {
        if self.until.is_none() {
            return Err(Refusal::OutOfPhase);
        }
        if replica != self.source {
            return Err(Refusal::WrongReplica);
        }
        if !self.draining_now() {
            return if offset <= self.tracker.base() {
                Ok(Next::Repeat)
            } else {
                Err(Refusal::OutOfPhase)
            };
        }
        match self.tracker.mark_committed(DRAIN.get(), offset) {
            Ok(Commit::Duplicate) => Ok(Next::Repeat),
            Ok(Commit::Advanced) => Ok(Next::Same),
            Err(offset_machines::Refusal::Unacked) => Err(Refusal::Unacked),
            Err(
                offset_machines::Refusal::Stale
                | offset_machines::Refusal::OutOfWindow
                | offset_machines::Refusal::Exhausted,
            ) => Err(untracked()),
        }
    }

    pub fn captured(&mut self, replica: u64, checkpoint: u64) -> Result<Next, Refusal> {
        if let Some(recorded) = self.checkpoint {
            return replay(replica == self.source, recorded == checkpoint);
        }
        let waiting = matches!(self.step, Step::Capture { .. });
        Self::expect(waiting, replica, self.source)?;
        self.checkpoint = Some(checkpoint);
        self.step = Step::Install { checkpoint };
        Ok(Next::Moved)
    }

    pub fn installed(&mut self, replica: u64, checkpoint: u64) -> Result<Next, Refusal> {
        if self.installed {
            let same = self.checkpoint == Some(checkpoint);
            return replay(replica == self.target, same);
        }
        let waiting = matches!(self.step, Step::Install { .. });
        Self::expect(waiting, replica, self.target)?;
        if self.checkpoint != Some(checkpoint) {
            return Err(Refusal::Mismatch);
        }
        self.installed = true;
        self.step = Step::Fence;
        Ok(Next::Moved)
    }

    pub fn fenced(&mut self, token: FencingToken) -> Result<Next, Refusal> {
        self.homed(token)?;
        if let Some(recorded) = self.token {
            return replay(true, recorded == token);
        }
        if self.step != Step::Fence {
            return Err(Refusal::OutOfPhase);
        }
        supersedes(token, self.source_token)?;
        self.token = Some(token);
        self.step = Step::Activate { token };
        Ok(Next::Moved)
    }

    pub fn activated(&self, replica: u64, token: FencingToken) -> Result<Next, Refusal> {
        self.homed(token)?;
        let waiting = matches!(self.step, Step::Activate { .. });
        Self::expect(waiting, replica, self.target)?;
        if self.token != Some(token) {
            return Err(Refusal::Mismatch);
        }
        Ok(Next::Done(Phase::Complete))
    }

    pub fn abort(&mut self, token: Option<FencingToken>) -> Result<Next, Refusal> {
        if let Some(token) = token {
            self.homed(token)?;
        }
        let discard = match self.step {
            Step::Rollback {
                token: recorded, ..
            } => return replay(true, recorded == token),
            Step::Quiesce | Step::Drain { .. } | Step::Capture { .. } => false,
            Step::Install { .. } | Step::Fence | Step::Activate { .. } => true,
        };
        // The activation token is of the source token's scope: `fenced`
        // admitted it. So the floor always names the transfer's one scope.
        let floor = self.token.unwrap_or(self.source_token);
        if let Some(token) = token {
            supersedes(token, floor)?;
        }
        if token.is_none() && matches!(self.step, Step::Activate { .. }) {
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
