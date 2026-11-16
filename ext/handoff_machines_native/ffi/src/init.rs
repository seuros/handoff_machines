//! Private native entry point.
use crate::values::{self, Family};
use crate::{CAPACITY, Core, Handoff, classes, names, scope_of};
use magnus::{Error, Ruby, function, method, prelude::*};
use matryona_machines::ruby::{Class, Symbols, fencing};

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("HandoffMachines")?;
    classes::resolve(ruby, module)?;
    module.const_set("CAPACITY", CAPACITY)?;
    module.const_set("WINDOW", Core::WINDOW)?;
    module.const_set("PHASES", names::Phases::every(ruby))?;
    module.const_set("REFUSALS", names::Refusals::every(ruby))?;
    let raw = function!(fencing::validate_raw::<Family>, 1);
    module.define_singleton_method("native_raw", raw)?;
    let scope = function!(fencing::token_scope::<Family>, 1);
    module.define_singleton_method("native_token_scope", scope)?;
    let order = function!(fencing::token_order::<Family>, 2);
    module.define_singleton_method("native_token_order", order)?;
    let newer = function!(fencing::token_newer::<Family>, 2);
    module.define_singleton_method("native_token_newer", newer)?;
    module.define_singleton_method("__limits", function!(values::check_limits, 2))?;
    module.define_singleton_method("__ticket", function!(values::check_ticket, 2))?;
    module.define_singleton_method("__recorded", function!(values::check_recorded, 1))?;
    module.define_singleton_method("__duplicate", function!(values::check_duplicate, 1))?;
    module.define_singleton_method("__replica", function!(values::check_replica, 1))?;
    module.define_singleton_method("__drain", function!(values::check_drain, 1))?;
    module.define_singleton_method("__capture", function!(values::check_capture, 1))?;
    module.define_singleton_method("__install", function!(values::check_install, 1))?;
    let token = function!(fencing::own_token::<Family>, 1);
    module.define_singleton_method("__token", token)?;
    module.define_singleton_method("__order", function!(values::check_order, 3))?;
    module.define_singleton_method("__status", function!(values::check_status, 9))?;
    let class = module.define_class("Handoff", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("native_with_scope", function!(Handoff::with_scope, 2))?;
    class.define_singleton_method("native_restored", function!(Handoff::restored, 3))?;
    class.define_singleton_method("native_scope_of", function!(scope_of, 1))?;
    class.define_method("scope", method!(Handoff::scope, 0))?;
    class.define_private_method("native_limits", method!(Handoff::limits, 0))?;
    class.define_method("start", method!(Handoff::start, 6))?;
    class.define_method("quiesced", method!(Handoff::quiesced, 4))?;
    class.define_method("drained", method!(Handoff::drained, 4))?;
    class.define_method("committed", method!(Handoff::committed, 4))?;
    class.define_method("captured", method!(Handoff::captured, 4))?;
    class.define_method("installed", method!(Handoff::installed, 4))?;
    class.define_method("fenced", method!(Handoff::fenced, 3))?;
    class.define_method("activated", method!(Handoff::activated, 4))?;
    class.define_method("abort", method!(Handoff::abort, 3))?;
    class.define_method("resumed", method!(Handoff::resumed, 3))?;
    class.define_method("discarded", method!(Handoff::discarded, 3))?;
    class.define_private_method("native_status", method!(Handoff::status, 1))?;
    class.define_private_method("native_orders", method!(Handoff::orders, 2))?;
    class.define_method("outstanding", method!(Handoff::outstanding, 3))?;
    class.define_private_method("native_overdue", method!(Handoff::overdue, 3))?;
    class.define_method("active", method!(Handoff::active, 0))?;
    class.define_method("generation", method!(Handoff::generation, 0))?;
    // `native_with_scope` and `native_restored` answer a Handoff: its class
    // is fixed now, before either runs.
    Class::pin_wrapped::<Handoff>(ruby, class).map(drop)
}
