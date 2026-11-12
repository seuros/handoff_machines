//! Private native entry point.
use crate::{CAPACITY, Coordinator, Core, names};
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
    let class = module.define_class("Coordinator", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("new", function!(Coordinator::new, -1))?;
    class.define_private_method("native_start", method!(Coordinator::start, 5))?;
    class.define_private_method("native_quiesced", method!(Coordinator::quiesced, 5))?;
    class.define_private_method("native_drained", method!(Coordinator::drained, 5))?;
    class.define_private_method("native_committed", method!(Coordinator::committed, 5))?;
    class.define_private_method("native_captured", method!(Coordinator::captured, 5))?;
    class.define_private_method("native_installed", method!(Coordinator::installed, 5))?;
    class.define_private_method("native_fenced", method!(Coordinator::fenced, 4))?;
    class.define_private_method("native_activated", method!(Coordinator::activated, 5))?;
    class.define_private_method("native_abort", method!(Coordinator::abort, 4))?;
    class.define_private_method("native_resumed", method!(Coordinator::resumed, 4))?;
    class.define_private_method("native_discarded", method!(Coordinator::discarded, 4))?;
    class.define_private_method("native_status", method!(Coordinator::status, 1))?;
    class.define_private_method("native_orders", method!(Coordinator::orders, 2))?;
    class.define_private_method("native_outstanding", method!(Coordinator::outstanding, 4))?;
    class.define_private_method("native_overdue", method!(Coordinator::overdue, 3))?;
    class.define_method("active", method!(Coordinator::active, 0))?;
    class.define_method("generation", method!(Coordinator::generation, 0))?;
    Ok(())
}
