# NON DEFICIMUS

> *"We do not fail."*

A Command & Conquer-style real-time strategy game for your terminal, built with Rust and [Ratatui](https://ratatui.rs).

The *NDS Non Deficimus*, a battered repair-and-recovery ship, falls out of a collapsing jump lane into a dead sector. You're the Captain. Harvest **Driftglass** to survive, but every crystal you pull wakes the **Hollow**, the derelict machines that corrode everything they touch.

## Status

Early prototype. The core loop is playable; Beacons, structures, more unit types and multiplayer are still on the roadmap.

## Run it

You need a recent [Rust toolchain](https://rustup.rs) and a terminal with mouse reporting and color (Windows Terminal, iTerm2 and most Linux terminals work, including over SSH).

```sh
cargo run --release
```

A terminal of at least 100x30 is recommended.

## How to play

**Hold and push.** Harvesting raises the sector's **Wake** meter. When Wake crosses a threshold (or time runs out), a Hollow **surge** hits your base, with a few seconds of warning. Survive three surges to hold the lane.

| Crew | Glyph | Role |
| --- | --- | --- |
| Cadet | `c` | Harvests Driftglass automatically; cheap and fragile |
| Engineer | `e` | Repairs crew and hull, and recovers Down units |
| Marine | `m` | Frontline fighter |

- **Decay:** Hollow attacks lower a target's *maximum* Integrity until an Engineer mends it.
- **Down, not dead:** a crewmate at zero Integrity can be recovered by an Engineer within about 30 seconds. If the Hollow reach them first, they're reclaimed as Hollow.
- **Resolve:** crew morale. It rises when you recover units and clear surges, and falls when you lose crew. At zero, the mission is lost.

### Controls

| Input | Action |
| --- | --- |
| Left-drag | Box-select crew |
| Right-click (ground) | Move selected crew (cadets on Driftglass or the ship resume harvesting) |
| Right-click (a Hollow) | Marines focus-fire that target |
| `a`, then click | Attack-move: Marines advance and engage anything they meet |
| `h` | Hold position: stay put and shoot |
| `1` `2` `3` | Train a Cadet, Engineer or Marine |
| `c` | Select all crew |
| Arrows | Pan the map |
| `Esc` | Deselect (also cancels a pending attack-move) |
| `Space` | Pause |
| `q` | Quit |

## Development

```sh
cargo test        # headless simulation tests
cargo run         # debug build
```

The simulation is deterministic (fixed 10 Hz tick, seeded RNG), which keeps lockstep multiplayer and replays possible down the road.

## Roadmap

- [ ] Beacons to relight, and structures to build
- [ ] Pilots and Systems Officers
- [ ] Mission scripting and a Captain's log campaign
- [ ] Co-op over the network
- [ ] Hollow as a playable side

## License

See [LICENSE](LICENSE).
