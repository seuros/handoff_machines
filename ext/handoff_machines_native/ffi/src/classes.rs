//! The `Data` classes, resolved once at load, the rule every value read from
//! Ruby passes, and clone-then-keep for every call whose answer runs Ruby.
//!
//! A value read back is either an instance of exactly this gem's class, whose
//! fields are read straight from the struct (no reader runs), or a sibling
//! gem's value of the same shape, read by its public readers. A subclass of
//! this gem's class is neither: the constant leaks past the seal and
//! `Data#initialize` can be bound directly, so it is a `TypeError`. Whatever
//! was read is then rebuilt through the core constructor, which `Marshal.load`
//! skipped.
use std::cell::RefCell;
use std::sync::OnceLock;

use magnus::rb_sys::AsRawValue;
use magnus::typed_data::Obj;
use magnus::value::Opaque;
use magnus::{Error, RClass, RModule, RStruct, Ruby, TypedData, Value, prelude::*};

/// Every `Data` class the binding checks or builds.
pub struct Classes {
    pub token: Opaque<RClass>,
    pub limits: Opaque<RClass>,
    pub ticket: Opaque<RClass>,
    pub recorded: Opaque<RClass>,
    pub duplicate: Opaque<RClass>,
    pub replica: Opaque<RClass>,
    /// The action classes, in [`ACTIONS`] order.
    pub actions: [Opaque<RClass>; 6],
}

/// The action names whose classes [`Classes::actions`] holds, in order.
pub const ACTIONS: [&str; 6] = ["Drain", "Capture", "Install", "Fence", "Activate", "Resume"];

static CLASSES: OnceLock<Classes> = OnceLock::new();

fn setup_error(ruby: &Ruby) -> Error {
    Error::new(
        ruby.exception_runtime_error(),
        "load handoff_machines, not the extension alone",
    )
}

/// Resolve the classes `handoff_machines.rb` defined before requiring the
/// extension. Each is registered with the GC, which keeps it alive and
/// pinned: compaction must not move a class the binding holds by address.
pub fn resolve(ruby: &Ruby, module: RModule) -> Result<(), Error> {
    let class = |name: &str| -> Result<Opaque<RClass>, Error> {
        let class: RClass = module.const_get(name).map_err(|_| setup_error(ruby))?;
        ruby.gc_register_mark_object(class);
        Ok(Opaque::from(class))
    };
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
    CLASSES.set(classes).map_err(|_| setup_error(ruby))
}

/// The classes resolved at load.
pub fn classes(ruby: &Ruby) -> Result<&'static Classes, Error> {
    CLASSES.get().ok_or_else(|| setup_error(ruby))
}

/// Whether `value` is an instance of exactly `class`, never a subclass, by
/// pointer equality (no `==` the caller could redefine runs). A singleton
/// class on the instance itself does not change what it is.
pub fn is(ruby: &Ruby, value: Value, class: Opaque<RClass>) -> bool {
    let (class, actual) = (ruby.get_inner(class), value.class());
    if actual.as_raw() == class.as_raw() {
        return true;
    }
    RStruct::from_value(value).is_some_and(|data| {
        data.singleton_class()
            .is_ok_and(|own| own.as_raw() == actual.as_raw())
            && actual
                .superclass()
                .is_ok_and(|parent| parent.as_raw() == class.as_raw())
    })
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
    class: Opaque<RClass>,
    readers: [&str; N],
    origin: Origin,
) -> Result<[Value; N], Error> {
    let exact = is(ruby, value, class);
    let class = ruby.get_inner(class);
    let mut out = [ruby.qnil().as_value(); N];
    if exact {
        let data = RStruct::from_value(value)
            .ok_or_else(|| type_error(ruby, format!("{class} is not a Data class")))?;
        for (index, slot) in out.iter_mut().enumerate() {
            *slot = data.aref(index)?;
        }
        return Ok(out);
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

/// Clone-then-keep: `decide` runs on a copy of the core and `build` turns its
/// answer into Ruby, with no borrow held. The copy is stored only when the
/// receiver is still unfrozen (`FrozenError`) and unchanged (`RuntimeError`:
/// a re-entrant call changed it); otherwise nothing is applied.
pub fn keep<T, C, A>(
    ruby: &Ruby,
    this: Obj<T>,
    core: &RefCell<C>,
    decide: impl FnOnce(&mut C) -> A,
    build: impl FnOnce(A) -> Result<Value, Error>,
) -> Result<Value, Error>
where
    T: TypedData,
    C: Clone + PartialEq,
{
    this.check_frozen()?;
    let before = core.borrow().clone();
    let mut after = before.clone();
    let answer = build(decide(&mut after))?;
    this.check_frozen()?;
    if *core.borrow() != before {
        return Err(Error::new(
            ruby.exception_runtime_error(),
            "the receiver changed while the answer was being built; the call was not applied",
        ));
    }
    *core.borrow_mut() = after;
    Ok(answer)
}
