//! Balance harness: scripted bots play full games headlessly so tuning
//! changes can be judged by numbers instead of feel.
//!
//! Run the report with:
//!
//! ```text
//! cargo test --release balance_report -- --ignored --nocapture
//! ```
//!
//! The bots use only `Command`s, like a human. They are deliberately
//! mediocre: if a mediocre bot wins sometimes, a good human should win
//! more often. The `balance_targets_hold` test pins the "hard but fair"
//! targets so a retune can't silently break them.

use crate::sim::*;

const SEEDS: u64 = 40;
/// Give up after 30 minutes of game time.
const MAX_TICKS: u64 = 18_000;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Bot {
    /// Never issues a command.
    Idle,
    /// Trains marines and cadets only; never leaves the ship.
    Turtle,
    /// Builds a balanced crew and relights Beacons between surges.
    Player,
    /// Like Turtle, but spends on an Engineer and turrets around the ship.
    Builder,
    /// Trains nothing and sends the Engineer straight for the Beacons,
    /// ignoring surges: the "rush" a first-time player tends to try.
    Rush,
}

#[derive(Default, Clone, Copy)]
struct Outcome {
    won: bool,
    lost: bool,
    ticks: u64,
    surges_cleared: u32,
    beacons_lit: usize,
    hull_at_end: i32,
    /// Lost because Resolve ran out (as opposed to the hull).
    resolve_loss: bool,
    last_stand: bool,
}

fn count(g: &Game, k: Kind) -> usize {
    g.units.iter().filter(|u| u.kind == k).count()
}

/// Once reasonably defended, hold back enough Driftglass for a relight.
fn reserve_for_beacons(g: &Game, marines: usize) -> i32 {
    if marines >= 5 && g.beacons.iter().any(|b| !b.lit) {
        RELIGHT_COST
    } else {
        0
    }
}

/// Try spots around the ship until a turret is accepted.
fn place_turret(g: &mut Game) {
    const SPOTS: [(i32, i32); 8] = [
        (7, 0),
        (-7, 0),
        (0, -6),
        (0, 6),
        (6, 5),
        (-6, -5),
        (6, -5),
        (-6, 5),
    ];
    let before = g.turrets.len();
    for (dx, dy) in SPOTS {
        g.apply(Command::Build((SHIP.0 + dx, SHIP.1 + dy)));
        if g.turrets.len() > before {
            return;
        }
    }
}

fn train_policy(g: &mut Game, bot: Bot) {
    let (cads, engs, marines) = (
        count(g, Kind::Cadet),
        count(g, Kind::Engineer),
        count(g, Kind::Marine),
    );
    match bot {
        Bot::Idle | Bot::Rush => {}
        Bot::Turtle => {
            if cads < 3 && g.glass >= g.train_cost(Kind::Cadet) {
                g.apply(Command::Train(Kind::Cadet));
            } else if g.glass >= g.train_cost(Kind::Marine) {
                g.apply(Command::Train(Kind::Marine));
            }
        }
        Bot::Builder => {
            if cads < 3 && g.glass >= g.train_cost(Kind::Cadet) {
                g.apply(Command::Train(Kind::Cadet));
            } else if marines < 3 && g.glass >= g.train_cost(Kind::Marine) {
                g.apply(Command::Train(Kind::Marine));
            } else if engs < 1 && g.glass >= g.train_cost(Kind::Engineer) {
                g.apply(Command::Train(Kind::Engineer));
            } else if g.turrets.len() < TURRET_CAP
                && g.turrets.iter().all(|t| t.built)
                && g.glass >= TURRET_COST
            {
                place_turret(g);
            } else if g.glass >= g.train_cost(Kind::Marine) {
                g.apply(Command::Train(Kind::Marine));
            }
        }
        Bot::Player => {
            if cads < 3 && g.glass >= g.train_cost(Kind::Cadet) {
                g.apply(Command::Train(Kind::Cadet));
            } else if marines < 3 && g.glass >= g.train_cost(Kind::Marine) {
                g.apply(Command::Train(Kind::Marine));
            } else if cads < 4 && g.glass >= g.train_cost(Kind::Cadet) {
                g.apply(Command::Train(Kind::Cadet));
            } else if engs < 2 && g.glass >= g.train_cost(Kind::Engineer) {
                g.apply(Command::Train(Kind::Engineer));
            } else if marines >= 4
                && g.turrets.len() < TURRET_CAP
                && g.turrets.iter().all(|t| t.built)
                && g.glass >= TURRET_COST + reserve_for_beacons(g, marines)
            {
                place_turret(g);
            } else if marines < 20
                && g.glass >= g.train_cost(Kind::Marine) + reserve_for_beacons(g, marines)
            {
                g.apply(Command::Train(Kind::Marine));
            }
        }
    }
}

/// Select exactly the units standing on `pos` and order them somewhere.
fn order_unit_at(g: &mut Game, pos: (i32, i32), cmd: Command) {
    g.apply(Command::SelectBox { min: pos, max: pos });
    g.apply(cmd);
    g.apply(Command::Deselect);
}

fn beacon_policy(g: &mut Game, bot: Bot) {
    // "busy" = a surge is announced or in flight
    let busy = bot != Bot::Rush && (g.surge.is_some() || g.surge_live);
    let defended = bot == Bot::Rush || count(g, Kind::Marine) >= 5;
    let home = (SHIP.0 + 3, SHIP.1 + 2);
    let engineers: Vec<(i32, i32)> = g
        .units
        .iter()
        .filter(|u| u.kind == Kind::Engineer && u.down.is_none())
        .map(|u| u.pos)
        .collect();
    if busy {
        // all hands home for a surge
        for p in engineers {
            if cheb(p, SHIP) > 6 {
                order_unit_at(g, p, Command::Move(home));
            }
        }
        return;
    }
    // one engineer works the next dark Beacon, if we can afford it
    if !defended {
        return;
    }
    let Some(k) = g.beacons.iter().position(|b| !b.lit) else {
        return;
    };
    let b = &g.beacons[k];
    if !(b.paid || g.glass >= RELIGHT_COST) {
        return;
    }
    let bp = b.pos;
    let near = |p: &(i32, i32)| cheb(*p, bp) <= 1;
    if engineers.iter().any(near) {
        return; // someone is already there
    }
    // send the engineer closest to idle duty: the first one that is idle
    if let Some(e) = g
        .units
        .iter()
        .find(|u| u.kind == Kind::Engineer && u.down.is_none() && u.order == Order::Idle)
        .map(|u| u.pos)
    {
        order_unit_at(g, e, Command::Move(bp));
    }
}

fn play_game(seed: u64, bot: Bot) -> Game {
    let mut g = Game::new(seed);
    for t in 0..MAX_TICKS {
        if t % 10 == 0 {
            train_policy(&mut g, bot);
        }
        if matches!(bot, Bot::Player | Bot::Rush) && t % 10 == 5 {
            beacon_policy(&mut g, bot);
        }
        g.update();
        if g.status != Status::Playing {
            break;
        }
    }
    g
}

fn play(seed: u64, bot: Bot) -> Outcome {
    let g = play_game(seed, bot);
    Outcome {
        won: g.status == Status::Won,
        lost: g.status == Status::Lost,
        ticks: g.tick,
        surges_cleared: g.surges_cleared,
        beacons_lit: g.beacons_lit(),
        hull_at_end: g.hull,
        resolve_loss: g.status == Status::Lost && g.resolve <= 0,
        last_stand: g.last_stand_used,
    }
}

struct Summary {
    wins: usize,
    losses: usize,
    timeouts: usize,
    resolve_losses: usize,
    last_stands: usize,
    median_ticks: u64,
    avg_surges: f32,
    avg_beacons: f32,
    avg_hull: f32,
}

fn run_all(bot: Bot) -> Summary {
    let outs: Vec<Outcome> = (0..SEEDS).map(|s| play(s * 7 + 3, bot)).collect();
    let n = outs.len() as f32;
    let mut ticks: Vec<u64> = outs.iter().map(|o| o.ticks).collect();
    ticks.sort_unstable();
    Summary {
        wins: outs.iter().filter(|o| o.won).count(),
        losses: outs.iter().filter(|o| o.lost).count(),
        timeouts: outs.iter().filter(|o| !o.won && !o.lost).count(),
        resolve_losses: outs.iter().filter(|o| o.resolve_loss).count(),
        last_stands: outs.iter().filter(|o| o.last_stand).count(),
        median_ticks: ticks[ticks.len() / 2],
        avg_surges: outs.iter().map(|o| o.surges_cleared as f32).sum::<f32>() / n,
        avg_beacons: outs.iter().map(|o| o.beacons_lit as f32).sum::<f32>() / n,
        avg_hull: outs.iter().map(|o| o.hull_at_end as f32).sum::<f32>() / n,
    }
}

#[test]
#[ignore]
fn balance_report() {
    println!(
        "\n{SEEDS} seeds per bot, up to {} min of game time",
        MAX_TICKS / 600
    );
    for bot in [Bot::Idle, Bot::Turtle, Bot::Builder, Bot::Player, Bot::Rush] {
        let s = run_all(bot);
        println!(
            "{bot:>7?}: won {:>2}  lost {:>2} (resolve {:>2})  timeout {:>2} | median {:>3}s | \
             surges {:>4.1}  beacons {:>3.1}  hull {:>5.1} | last stands {:>2}",
            s.wins,
            s.losses,
            s.resolve_losses,
            s.timeouts,
            s.median_ticks / 10,
            s.avg_surges,
            s.avg_beacons,
            s.avg_hull,
            s.last_stands,
        );
    }
}

/// Print how a few games end: `cargo test --release balance_trace -- --ignored --nocapture`
#[test]
#[ignore]
fn balance_trace() {
    for seed in [3u64, 10, 17] {
        let g = play_game(seed, Bot::Player);
        println!(
            "
--- seed {seed}: {:?} at {}s, surges cleared {}, beacons {}, crew {}, resolve {}, hull {}",
            g.status,
            g.tick / 10,
            g.surges_cleared,
            g.beacons_lit(),
            g.units.len(),
            g.resolve,
            g.hull
        );
        for line in g.log.iter().rev().take(14).rev() {
            println!("   {line}");
        }
    }
}

/// Fast guard (runs in normal `cargo test`): a player who does nothing
/// still gets a quiet opening before the first surge.
#[test]
fn opening_is_quiet_for_the_first_two_minutes() {
    for seed in 0..10 {
        let mut g = Game::new(seed * 7 + 3);
        for _ in 0..1_200 {
            g.update();
        }
        assert_eq!(g.surges_launched, 0, "seed {seed}: surge came too early");
    }
}

/// The "hard but fair" floor. Slow in debug builds, so CI runs it with
/// `cargo test --release balance_targets_hold -- --ignored`.
///
/// These are deliberately loose: the bots are mediocre, so this checks that
/// the game is neither hopeless nor trivial, not that it is perfectly tuned.
#[test]
#[ignore = "slow; run with --release -- --ignored"]
fn balance_targets_hold() {
    let idle = run_all(Bot::Idle);
    assert_eq!(idle.wins, 0, "doing nothing must never win");
    assert!(
        idle.median_ticks >= 1_500,
        "opening should give ~3 min before the first surge lands (median {}s)",
        idle.median_ticks / 10
    );

    let turtle = run_all(Bot::Turtle);
    assert!(
        turtle.median_ticks >= 6_000,
        "a pure defence should hold for ~10 min (median {}s)",
        turtle.median_ticks / 10
    );

    // A turret-building crew should hold at least as well as a pure Marine one.
    let builder = run_all(Bot::Builder);
    assert!(
        builder.losses <= turtle.losses,
        "turrets should help (builder lost {} of {SEEDS}, turtle {})",
        builder.losses,
        turtle.losses
    );

    // Winnable but not trivial, even for a mediocre bot (it wins roughly a
    // third of games at the time of writing; a human should do better).
    let player = run_all(Bot::Player);
    assert!(
        player.wins >= 3,
        "winning must be possible (bot won {} of {SEEDS})",
        player.wins
    );
    assert!(
        player.wins <= SEEDS as usize * 3 / 4,
        "winning must not be trivial (bot won {} of {SEEDS})",
        player.wins
    );
    // Rushing the lights with no army must not be a way to skip the game.
    let rush = run_all(Bot::Rush);
    assert!(
        rush.wins <= SEEDS as usize / 10,
        "an army-less rush should almost never win (won {} of {SEEDS})",
        rush.wins
    );
    assert!(
        player.median_ticks >= 3_000,
        "a crew working the Beacons should last ~5 min (median {}s)",
        player.median_ticks / 10
    );
}
