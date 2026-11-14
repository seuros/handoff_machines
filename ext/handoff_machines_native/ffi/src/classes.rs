//! The `Data` classes, resolved once at load and pinned. The exact-class
//! check, the own-or-sibling read (see `values`) and clone-then-keep are
//! `matryona_machines::ruby`'s.
use magnus::{Error, RModule, Ruby};
use matryona_machines::ruby::{Class, Registry};

/// Every `Data` class the binding checks or builds.
pub struct Classes {
    pub token: Class,
    pub limits: Class,
    pub ticket: Class,
    pub recorded: Class,
    pub duplicate: Class,
    pub replica: Class,
    /// The action classes, in [`ACTIONS`] order.
    pub actions: [Class; 6],
}

/// The action names whose classes [`Classes::actions`] holds, in order.
pub const ACTIONS: [&str; 6] = ["Drain", "Capture", "Install", "Fence", "Activate", "Resume"];

static CLASSES: Registry<Classes> = Registry::new("handoff_machines");

/// Resolve the classes `handoff_machines.rb` defined before requiring the
/// extension. Called once, from init; never by name again.
pub fn resolve(ruby: &Ruby, module: RModule) -> Result<(), Error> {
    let class = |name| Class::resolve(ruby, module, name);
    let [drain, capture, install, fence, activate, resume] = ACTIONS;
    let classes = Classes {
        token: class("FencingToken")?,
        limits: class("Limits")?,
        ticket: class("Ticket")?,
        recorded: class("Recorded")?,
        duplicate: class("Duplicate")?,
        replica: class("Replica")?,
        actions: [
            class(drain)?,
            class(capture)?,
            class(install)?,
            class(fence)?,
            class(activate)?,
            class(resume)?,
        ],
    };
    CLASSES.set(ruby, classes).map(drop)
}

/// The classes resolved at load.
pub fn classes(ruby: &Ruby) -> Result<&'static Classes, Error> {
    CLASSES.get(ruby)
}
