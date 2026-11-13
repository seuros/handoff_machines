//! Private native entry point.
use crate::{CAPACITY, Core, Handoff, names};
use magnus::{Error, Ruby, function, method, prelude::*};

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("HandoffMachines")?;
    module.const_set("CAPACITY", CAPACITY)?;
    module.const_set("WINDOW", Core::WINDOW)?;
    let symbols = |names: &[&str]| {
        let list = ruby.ary_from_iter(names.iter().map(|name| ruby.to_symbol(*name)));
        list.freeze();
        list
    };
    module.const_set("PHASES", symbols(&names::PHASES))?;
    module.const_set("REFUSALS", symbols(&names::REFUSALS))?;
    let class = module.define_class("Handoff", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("native_restored", function!(Handoff::restored, 3))?;
    class.define_private_method("native_limits", method!(Handoff::limits, 0))?;
    class.define_private_method("native_start", method!(Handoff::start, 5))?;
    class.define_private_method("native_quiesced", method!(Handoff::quiesced, 5))?;
    class.define_private_method("native_drained", method!(Handoff::drained, 5))?;
    class.define_private_method("native_committed", method!(Handoff::committed, 5))?;
    class.define_private_method("native_captured", method!(Handoff::captured, 5))?;
    class.define_private_method("native_installed", method!(Handoff::installed, 5))?;
    class.define_private_method("native_fenced", method!(Handoff::fenced, 4))?;
    class.define_private_method("native_activated", method!(Handoff::activated, 5))?;
    class.define_private_method("native_abort", method!(Handoff::abort, 4))?;
    class.define_private_method("native_resumed", method!(Handoff::resumed, 4))?;
    class.define_private_method("native_discarded", method!(Handoff::discarded, 4))?;
    class.define_private_method("native_status", method!(Handoff::status, 1))?;
    class.define_private_method("native_orders", method!(Handoff::orders, 2))?;
    class.define_private_method("native_outstanding", method!(Handoff::outstanding, 4))?;
    class.define_private_method("native_overdue", method!(Handoff::overdue, 3))?;
    class.define_method("active", method!(Handoff::active, 0))?;
    class.define_method("generation", method!(Handoff::generation, 0))?;
    Ok(())
}
