# Changelog

## [0.3.0](https://github.com/seuros/handoff_machines/compare/handoff-machines-v0.2.0...handoff-machines-v0.3.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* see CONVENTIONS: one exhaustive Refusal per crate, Result from every mutator, honoured idempotent repeats only when provable, Duration time, Ruby mirrors the core. No compatibility aliases. Requires offset-machines >= 0.3. A replay with a different payload is Mismatch; the Ruby class is Handoff and takes Limits.

### Features

* every acknowledgement answers Result&lt;Acked, Refusal&gt;, honouring recorded repeats as Duplicate ([9d6f347](https://github.com/seuros/handoff_machines/commit/9d6f3472e5c84fd887d87eb4b9aa05b482ffd1e1))

## [0.2.0](https://github.com/seuros/handoff_machines/compare/handoff-machines-v0.1.0...handoff-machines-v0.2.0) (2026-11-12)


### Features

* initial extraction ([acfda8e](https://github.com/seuros/handoff_machines/commit/acfda8e11b70570a4728955c14a04bc31d5cbba3))
