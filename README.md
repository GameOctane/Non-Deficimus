# NON DEFICIMUS

> *"We do not fail."*

A Command & Conquer-style real-time strategy game for your terminal, built with Rust and [Ratatui](https://ratatui.rs).

The *NDS Non Deficimus*, a battered repair-and-recovery ship, falls out of a collapsing jump lane into a dead sector. You're the Captain. Harvest **Driftglass** to survive, but every crystal you pull wakes the **Hollow**, the derelict machines that corrode everything they touch.

## Status

Early prototype. The core loop and Beacons are playable; a defensive structure, more unit types and multiplayer are still on the roadmap.

## Run it

You need a recent [Rust toolchain](https://rustup.rs) and a terminal with mouse reporting and color (Windows Terminal, iTerm2 and most Linux terminals work, including over SSH).

```sh
cargo run --release
```

A terminal of at least 100x30 is recommended (the map title warns you if yours is smaller). On a low-resolution screen, maximize or full-screen the terminal and zoom the font out (Ctrl+minus, or Ctrl+scroll in Windows Terminal) until it fits; the map and comms log grow to fill whatever space you give them. You start with four crew and **can train more** with `1` `2` `3`; a briefing screen explains this when the game opens.

## How to play

**Hold and push.** Harvesting raises the sector's **Wake** meter. When Wake crosses a threshold (or time runs out), a Hollow **surge** hits your base, with about 15 seconds of warning. Surges keep getting bigger.

**Win:** relight all four **Beacons**, then survive the final surge. It is called about 30 seconds after the fourth light comes on (plus the usual warning), so you won't be left waiting. If a Beacon goes dark before the surge is cleared, you'll have to relight it and hold out for the next one. The sidebar always shows how long until the next surge at the latest; Wake can bring it sooner.

**Lose:** the hull is destroyed or Resolve hits zero.

### Beacons

Four dead Beacons sit around the sector, each farther from the ship than the last. They show as `o` (dark, dim red), `o` in bold yellow with a progress bar above it (being relit), and `*` (lit; it turns orange when corroded).

- An **Engineer** standing next to a Beacon relights it in about 8 seconds. It costs **30 Driftglass**, paid when the work starts. Right-clicking a Beacon sends the Engineer to its side.
- Each lit Beacon makes **Wake decay faster** and gives a small **Resolve** bonus.
- The Hollow will **corrode lit Beacons** when no crew are nearby. A Beacon that runs out of Integrity goes dark again, and the Resolve bonus goes with it.

| Crew | Glyph | Role |
| --- | --- | --- |
| Cadet | `c` | Harvests Driftglass automatically; cheap and fragile |
| Engineer | `e` | Repairs crew and hull, and recovers Down units |
| Marine | `m` | Frontline fighter |

- **Resolve tiers:** the sidebar shows how the crew is holding up, and it matters.
  - **70 or more (Steady, then NON DEFICIMUS at 90+):** Engineers work faster.
  - **31-69 (Holding):** no effect.
  - **30 or below (Frayed):** training costs 25% more.
  - **Last stand:** once per game, when the hull drops under 25% and the crew isn't Frayed, they find one more push. Everyone still standing is healed and the hull is shored up, but Resolve is knocked down to 60 if it was higher. The better their Resolve, the stronger it is: NON DEFICIMUS gives +50 hull and a full heal, Steady three quarters of that, Holding half. A Frayed crew has nothing left to give.
- **Decay:** Hollow attacks lower a target's *maximum* Integrity until an Engineer mends it.
- **Down, not dead:** a crewmate at zero Integrity can be recovered by an Engineer within about 30 seconds. If the Hollow reach them first, they're reclaimed as Hollow.
- **Resolve:** crew morale. It rises when you recover units and clear surges, and falls when you lose crew. At zero, the mission is lost.

### Controls

| Input | Action |
| --- | --- |
| Left-drag | Box-select crew |
| Right-click (ground) | Move selected crew (cadets on Driftglass or the ship resume harvesting) |
| Right-click (a Beacon) | Move to its side; an Engineer there relights it |
| Right-click (a Hollow) | Marines focus-fire that target |
| `a`, then click | Attack-move: Marines advance and engage anything they meet |
| `h` | Hold position: stay put and shoot |
| `1` `2` `3` | Train a Cadet, Engineer or Marine |
| `c` | Select all crew |
| Arrows | Pan the map |
| `Esc` | Deselect (also cancels a pending attack-move) |
| `PgUp` / `PgDn` | Scroll the comms log back / forward (`End` jumps to the latest; `Space` pauses so you can read) |
| `?` | Show the Captain's briefing again |
| `Space` | Pause |
| `q` | Quit |

## Development

```sh
cargo test        # headless simulation tests
cargo run         # debug build
```

The simulation is deterministic (fixed 10 Hz tick, seeded RNG), which keeps lockstep multiplayer and replays possible down the road.

## Roadmap

- [x] Beacons to relight
- [ ] A defensive structure to build
- [ ] Pilots and Systems Officers
- [ ] Mission scripting and a Captain's log campaign
- [ ] Co-op over the network
- [ ] Hollow as a playable side

## License

Copyright (C) 2026 Jason Germino (GameOctane).

Licensed under the GNU General Public License, version 3 or (at your option) any later version. See [LICENSE](LICENSE).
