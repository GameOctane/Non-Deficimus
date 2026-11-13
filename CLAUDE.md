# CLAUDE.md

NON DEFICIMUS: a terminal RTS prototype in Rust + Ratatui. The crew harvests Driftglass, which raises the Wake meter and triggers Hollow surges. Units at zero Integrity are Down and can be recovered by Engineers; otherwise the Hollow reclaim them. See [README.md](README.md) for the player-facing description.

## Commands

- `cargo build`
- `cargo test` (headless simulation tests)
- `cargo run --release`

## Rules

### Determinism
The simulation must stay deterministic.
- Fixed 10 Hz tick (`TICK_MS = 100`). One `update()` call is one tick.
- Seeded RNG only (the `Game::rand` xorshift). No `rand` crate, no OS entropy in sim code.
- No wall-clock time in sim code (`Instant`, `SystemTime`, etc.). Time exists only as tick counts. The seed is chosen outside the sim, in `run()`, and passed to `Game::new`.
- Player input enters the sim only through commands (train, order, hold, etc.), never by UI code poking sim state directly.
- Same seed + same commands at the same ticks must give identical results.

### Separation
Sim code must not import Ratatui or crossterm. Rendering, input decoding, camera and drag-box state belong in the UI layer.

### Testing and commits
- `cargo test` must pass before any commit. So must `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`; CI (`.github/workflows/ci.yml`) runs all three.
- Every new mechanic gets a headless test (no terminal, drive `Game` directly).

### Dependencies
Do not add dependencies without asking first.

### Tone for in-game text
Dry, tired, human Captain's-log voice. Understated, a little gallows humor, never heroic bombast. Example: "We do not fail." Keep messages short.

### Scope: v1 vertical slice
One map, 3 unit types (Cadet, Engineer, Marine), Beacons, one defensive structure, win/lose screen.

Do NOT add Pilots, Systems Officers, Drifters, multiplayer, or a campaign yet.

## Layout

- `src/sim.rs`: state, rules, pathing, `Command` and `Game::apply`. No Ratatui/crossterm. Tests live here.
- `src/ui.rs`: drawing and input decoding. `Ui` holds the camera, drag box and pending attack-move. Input handlers read `Game` but only return `Command`s.
- `src/main.rs`: terminal setup, the seed, and the real-time loop (applies commands, calls `update()` at 10 Hz).

## Balance

- Tuning knobs are named constants at the top of `src/sim.rs` (Wake pacing, surge timer and warning, Resolve, Beacon costs). Change those, not the rules.
- `src/balance.rs` is a headless harness: scripted bots (Idle, Turtle, Player) play full games using only `Command`s.
  - Report: `cargo test --release balance_report -- --ignored --nocapture`
  - Trace how games end: `cargo test --release balance_trace -- --ignored --nocapture`
  - Floor targets (CI runs this): `cargo test --release balance_targets_hold -- --ignored`
- Retune with the report, not by feel. Findings so far: a tight group of Marines beats a spread-out one; idle Marines chasing Hollow (even leashed) did worse than standing together; the Player bot wins only ~2% of games, so "winnable by a good human" is unproven and needs real playtesting.

## Not yet done

- The defensive structure is not implemented yet (README roadmap). Beacons are done.
