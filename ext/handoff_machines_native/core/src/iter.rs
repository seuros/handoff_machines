use core::time::Duration;

use offset_machines::{Gaps, Tracker};

use crate::{Order, Ticket, transfer::Transfer};

/// Resumable pass over outstanding orders; see [`crate::Handoff::orders`].
#[derive(Debug, Clone)]
pub struct Orders<'a, const WORDS: usize> {
    slots: &'a [Option<Transfer<WORDS>>],
    position: usize,
}

impl<'a, const WORDS: usize> Orders<'a, WORDS> {
    pub(crate) const fn new(slots: &'a [Option<Transfer<WORDS>>], from: usize) -> Self {
        Self {
            slots,
            position: from,
        }
    }

    /// Where a later pass continues after the orders yielded so far.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }
}

impl<const WORDS: usize> Iterator for Orders<'_, WORDS> {
    type Item = Order;

    fn next(&mut self) -> Option<Order> {
        while let Some(slot) = self.slots.get(self.position / 2) {
            let part = self.position % 2;
            self.position += 1;
            let order = slot.as_ref().and_then(|transfer| {
                let (to, action) = transfer.order(part)?;
                Some(Order {
                    ticket: transfer.ticket,
                    to,
                    action,
                })
            });
            if order.is_some() {
                return order;
            }
        }
        None
    }
}

/// Resumable pass over overdue transfers; see [`crate::Handoff::overdue`].
#[derive(Debug, Clone)]
pub struct Overdue<'a, const WORDS: usize> {
    slots: &'a [Option<Transfer<WORDS>>],
    now: Duration,
    position: usize,
}

impl<'a, const WORDS: usize> Overdue<'a, WORDS> {
    pub(crate) const fn new(
        slots: &'a [Option<Transfer<WORDS>>],
        now: Duration,
        from: usize,
    ) -> Self {
        Self {
            slots,
            now,
            position: from,
        }
    }

    /// Where a later pass continues after the tickets yielded so far.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }
}

impl<const WORDS: usize> Iterator for Overdue<'_, WORDS> {
    type Item = Ticket;

    fn next(&mut self) -> Option<Ticket> {
        while let Some(slot) = self.slots.get(self.position) {
            self.position += 1;
            if let Some(transfer) = slot
                && self.now >= transfer.deadline
            {
                return Some(transfer.ticket);
            }
        }
        None
    }
}

/// Offsets a drain still waits for; see [`crate::Handoff::outstanding`].
#[derive(Debug, Clone)]
pub struct Outstanding<'a, const WORDS: usize> {
    gaps: Gaps<'a, WORDS>,
    next: u64,
    until: u64,
}

impl<'a, const WORDS: usize> Outstanding<'a, WORDS> {
    pub(crate) fn new(tracker: &'a Tracker<WORDS>, from: u64, until: u64) -> Self {
        let frontier = tracker.frontier().unwrap_or_else(|| tracker.base());
        Self {
            gaps: tracker.gaps(from),
            next: from.max(frontier),
            until,
        }
    }
}

impl<const WORDS: usize> Iterator for Outstanding<'_, WORDS> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        if let Some(gap) = self.gaps.next() {
            return Some(gap);
        }
        (self.next < self.until).then(|| {
            let offset = self.next;
            self.next += 1;
            offset
        })
    }
}
