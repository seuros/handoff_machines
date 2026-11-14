//! Seeded draws and helpers for the randomized model test.
use std::{format, string::String, vec::Vec};

use crate::Order;
use periscope_machines::scoped;

pub use matryona_machines::testing::XorShift64 as Draws;

/// `true` `percent` times in a hundred.
pub fn chance(d: &mut Draws, percent: u64) -> bool {
    d.below(100) < percent
}

/// Token `sequence` as a raw value: of the run's token scope `home`, or one
/// time in twelve of a neighbouring scope, whose raw values lie above or below.
pub fn fencing(sequence: u64, home: u16, d: &mut Draws) -> u64 {
    let scope = match d.below(24) {
        0 => home.wrapping_add(1),
        1 => home.wrapping_sub(1),
        _ => home,
    };
    scoped(scope, sequence)
}

pub fn sorted(orders: impl IntoIterator<Item = Order>) -> Vec<String> {
    let mut lines: Vec<String> = orders.into_iter().map(|o| format!("{o:?}")).collect();
    lines.sort();
    lines
}
