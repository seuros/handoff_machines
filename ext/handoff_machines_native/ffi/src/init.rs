//! Private native entry point.
use crate::{
    CAPACITY, Core, Handoff, classes, names, scope_of, token_newer, token_order, token_scope,
    validate_raw, values,
};
use magnus::{Error, Ruby, function, method, prelude::*};

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("HandoffMachines")?;
    classes::resolve(ruby, module)?;
    module.const_set("CAPACITY", CAPACITY)?;
    module.const_set("WINDOW", Core::WINDOW)?;
    let symbols = |names: &[&str]| {
        let list = ruby.ary_from_iter(names.iter().map(|name| ruby.to_symbol(*name)));
        list.freeze();
        list
    };
    module.const_set("PHASES", symbols(&names::PHASES))?;
    module.const_set("REFUSALS", symbols(&names::REFUSALS))?;
    module.define_singleton_method("native_raw", function!(validate_raw, 1))?;
    module.define_singleton_method("native_token_scope", function!(token_scope, 1))?;
    module.define_singleton_method("native_token_order", function!(token_order, 2))?;
    module.define_singleton_method("native_token_newer", function!(token_newer, 2))?;
    module.define_singleton_method("__limits", function!(values::check_limits, 2))?;
    module.define_singleton_method("__ticket", function!(values::check_ticket, 2))?;
    module.define_singleton_method("__recorded", function!(values::check_recorded, 1))?;
    module.define_singleton_method("__duplicate", function!(values::check_duplicate, 1))?;
    module.define_singleton_method("__u64", function!(values::check_u64, 1))?;
    module.define_singleton_method("__token", function!(values::check_token, 1))?;
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
    Ok(())
}
