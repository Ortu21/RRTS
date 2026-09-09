# Product

<!-- impeccable:product-schema 1 -->

## Platform
Native desktop game: macOS, Windows and Linux. This is Bevy UI, not a web or mobile surface.

## Users and purpose
The developer plays a procedural RTS, observes bot matches, and diagnoses simulation behaviour. The interface must make these three activities comfortable without prioritising visual polish.

## Capabilities and constraints
Rust / Bevy 0.19, procedural graphics, existing CLI startup. Player commands, construction and factory production remain contextual. Observation never grants command authority. Human/bot ownership can transfer without replacing a team's existing orders. Debug tools are independent and initially disabled; private enemy information requires an explicit full-view override.

## Product principles
- Preserve the battlefield as the primary working area.
- Reveal actions according to selection and role.
- Separate diagnostic presentation from simulation decisions.
- Keep camera and diagnostics usable while paused or after match end.
- Retain the headless benchmark's determinism and measurement boundaries.
