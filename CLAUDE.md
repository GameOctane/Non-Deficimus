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
- `cargo test` must pass before any commit.
- Every new mechanic gets a headless test (no terminal, drive `Game` directly).

### Dependencies
Do not add dependencies without asking first.

### Tone for in-game text
Dry, tired, human Captain's-log voice. Understated, a little gallows humor, never heroic bombast. Example: "We do not fail." Keep messages short.

### Scope: v1 vertical slice
One map, 3 unit types (Cadet, Engineer, Marine), Beacons, one defensive structure, win/lose screen.

Do NOT add Pilots, Systems Officers, Drifters, multiplayer, or a campaign yet.

## Known gaps against these rules (as of the baseline)

All of the sim and UI currently live in `src/main.rs`, and some of it predates these rules. Fix these when touching the relevant code; don't spread them further.
- `Kind::color()` and `Game::cam_for` / `world_at` use Ratatui types (`Color`, `Rect`), and `Game` holds UI state (`cam`, `drag`, `paused`, `pending_amove`).
- `handle_mouse` / `handle_key` mutate `Game` directly rather than issuing commands.
- Beacons and the defensive structure are not implemented yet (README roadmap).
