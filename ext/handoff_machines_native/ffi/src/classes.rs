//! The `Data` classes, resolved once at load and pinned, and the rule every
//! value read from Ruby passes. The exact-class check and clone-then-keep
//! are `matryona_machines::ruby`'s.
//!
//! A value read back is either an instance of exactly this gem's class, whose
//! fields are read straight from the struct (no reader runs), or a sibling
//! gem's value of the same shape, read by its public readers. A subclass of
//! this gem's class is neither: the constant leaks past the seal and
//! `Data#initialize` can be bound directly, so it is a `TypeError`. Whatever
//! was read is then rebuilt through the core constructor, which `Marshal.load`
//! skipped.
use magnus::{Error, RClass, RModule, Ruby, Value, prelude::*};
use matryona_machines::ruby::{Class, Registry, exact_fields_with, instance_of};

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

fn type_error(ruby: &Ruby, message: String) -> Error {
    Error::new(ruby.exception_type_error(), message)
}

/// Who may hand the binding a value of a class: this gem only, or a sibling
/// gem's value of the same shape too.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Own,
    Sibling,
}

/// The fields of `value`, in `readers` order: from the struct when `value` is
/// exactly `class`, through its public readers when a sibling may hand it
/// over. `TypeError` for a subclass of `class` and for anything else.
pub fn fields<const N: usize>(
    ruby: &Ruby,
    value: Value,
    class: Class,
    readers: [&str; N],
    origin: Origin,
) -> Result<[Value; N], Error> {
    let class: RClass = class.get(ruby);
    if instance_of(value, class) {
        return exact_fields_with::<N>(ruby, value, class, || format!("expected a {class}"));
    }
    if value.is_kind_of(class) {
        return Err(type_error(
            ruby,
            format!("{class} is sealed: a subclass of it is not a {class}"),
        ));
    }
    if origin == Origin::Own {
        return Err(type_error(ruby, format!("expected a {class}")));
    }
    let mut out = [ruby.qnil().as_value(); N];
    for (slot, reader) in out.iter_mut().zip(readers) {
        if !value.respond_to(reader, false)? {
            return Err(type_error(
                ruby,
                format!("expected a {class} or a value with its readers (no #{reader})"),
            ));
        }
        *slot = value.funcall(reader, ())?;
    }
    Ok(out)
}
