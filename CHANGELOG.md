# Changelog

## [0.4.0](https://github.com/seuros/handoff_machines/compare/handoff_machines/v0.3.0...handoff_machines/v0.4.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* converted to the fleet conventions. One exhaustive Refusal per crate, Result from every operation that can refuse, a refusal changes nothing, provable idempotent repeats are honoured, Ruby mirrors the core. No compatibility aliases. Requires lease-machines >= 0.4. Ruby takes a token by its raw reader; a bare Integer is a TypeError.

### Features

* fencing tokens are scoped values; a token of another scope is Unknown before anything else is read ([77e57ef](https://github.com/seuros/handoff_machines/commit/77e57ef727f205497d89171dd03db9f2aa6578aa))

## [0.3.0](https://github.com/seuros/handoff_machines/compare/handoff_machines/v0.2.0...handoff_machines/v0.3.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* handles are issued through periscope-machines. Every instance can be built with a scope; a handle from another scope is refused before any lookup; all handle kinds of an instance share one sequence; resumed restores the sequence after a restart. restored(limits, scope, last_generation) returns an Option.

### Features

* scope ticket generations through periscope-machines; restored takes the scope and refuses a saved generation from another scope ([978b653](https://github.com/seuros/handoff_machines/commit/978b653d386062d925d002370930bf09d2d2dbf8))

## [0.2.0](https://github.com/seuros/handoff_machines/compare/handoff_machines/v0.1.1...handoff_machines/v0.2.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* see CONVENTIONS: one exhaustive Refusal per crate, Result from every mutator, honoured idempotent repeats only when provable, Duration time, Ruby mirrors the core. No compatibility aliases. Requires offset-machines >= 0.3. A replay with a different payload is Mismatch; the Ruby class is Handoff and takes Limits.

### Features

* every acknowledgement answers Result&lt;Acked, Refusal&gt;, honouring recorded repeats as Duplicate ([9d6f347](https://github.com/seuros/handoff_machines/commit/9d6f3472e5c84fd887d87eb4b9aa05b482ffd1e1))

## [0.1.1](https://github.com/seuros/handoff_machines/compare/handoff_machines-v0.1.0...handoff_machines/v0.1.1) (2026-11-12)


### Features

* initial extraction ([acfda8e](https://github.com/seuros/handoff_machines/commit/acfda8e11b70570a4728955c14a04bc31d5cbba3))
