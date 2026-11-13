//! The simulation: state, rules and pathing. Deterministic and headless.
//!
//! No rendering or terminal code lives here. Player input enters only
//! through [`Command`] and [`Game::apply`].

use std::collections::VecDeque;

pub const W: i32 = 96;
pub const H: i32 = 48;
pub const SHIP: (i32, i32) = (W / 2, H / 2);
/// Real-time length of one tick (10 Hz). The sim itself only counts ticks.
pub const TICK_MS: u64 = 100;
pub const MAX_CREW: usize = 24;

// Tuning knobs. Change these (and run the balance harness) rather than
// editing the rules.
/// Resolve the crew starts with.
pub const START_RESOLVE: i32 = 70;
/// Resolve tiers. At `RESOLVE_QUICK` or more, Engineers work faster; at
/// `RESOLVE_FRAYED` or less, training costs more; at `RESOLVE_UNBROKEN` or
/// more, a badly hurt ship makes a one-time last stand.
pub const RESOLVE_QUICK: i32 = 70;
pub const RESOLVE_UNBROKEN: i32 = 90;
pub const RESOLVE_FRAYED: i32 = 30;
/// Ticks between Engineer repair pulses, normally and at high Resolve.
pub const ENGINEER_PULSE_TICKS: u64 = 20;
pub const ENGINEER_PULSE_QUICK_TICKS: u64 = 15;
/// Training cost multiplier (percent) while Frayed.
pub const FRAYED_COST_PERCENT: i32 = 125;
/// Last stand: fires when Resolve is Unbroken and hull is under this share
/// of its base (1 in N), once per game.
pub const LAST_STAND_HULL_FRACTION: i32 = 4;
pub const LAST_STAND_HULL_BONUS: i32 = 50;
/// Minimum Resolve for the last stand.
pub const LAST_STAND_MIN_RESOLVE: i32 = RESOLVE_UNBROKEN;
/// Resolve the crew is left with after a last stand.
pub const LAST_STAND_RESOLVE: i32 = 60;
/// Resolve lost when a Down crewmate is reclaimed by the Hollow.
pub const RESOLVE_LOST_UNIT: i32 = 6;
/// A surge is forced this many ticks after the last one, whatever the Wake.
pub const SURGE_TIMER_TICKS: u64 = 1800;
/// Warning between a surge being announced and the Hollow arriving.
pub const SURGE_WARNING_TICKS: u32 = 150;
/// Wake needed for the first surge, and the extra needed for each later one.
pub const WAKE_TRIGGER_FIRST: f32 = 25.0;
pub const WAKE_TRIGGER_STEP: f32 = 12.0;
/// Wake gained per Driftglass delivered.
pub const WAKE_PER_GLASS: f32 = 0.15;

/// Hollow contacts in a surge, given how many have launched and the Wake.
pub fn surge_size(launched: u32, wake: f32) -> u32 {
    2 + launched + (wake / 25.0) as u32
}
pub const BEACON_COUNT: usize = 4;
/// Ticks an Engineer must tend a Beacon to relight it (~8 s).
pub const RELIGHT_TICKS: u32 = 80;
/// Driftglass spent when a relight begins.
pub const RELIGHT_COST: i32 = 30;
pub const BEACON_HP: i32 = 30;
/// Resolve gained when a Beacon is relit (and lost again if it is destroyed).
pub const BEACON_RESOLVE: i32 = 3;
/// Chebyshev distance bands from the ship, one per Beacon, nearest first.
const BEACON_BANDS: [(i32, i32); BEACON_COUNT] = [(8, 12), (16, 22), (24, 32), (36, 44)];

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Cadet,
    Engineer,
    Marine,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Cadet => "cadet",
            Kind::Engineer => "engineer",
            Kind::Marine => "marine",
        }
    }
    pub fn glyph(self) -> char {
        match self {
            Kind::Cadet => 'c',
            Kind::Engineer => 'e',
            Kind::Marine => 'm',
        }
    }
    pub fn cost(self) -> i32 {
        match self {
            Kind::Cadet => 20,
            Kind::Engineer => 35,
            Kind::Marine => 40,
        }
    }
    pub fn hp(self) -> i32 {
        match self {
            Kind::Cadet => 12,
            Kind::Engineer => 14,
            Kind::Marine => 24,
        }
    }
}

/// How the crew is holding up. Each tier has a mechanical effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolveTier {
    /// 30 or below: training costs 25% more.
    Frayed,
    /// 31-69: no modifiers.
    Holding,
    /// 70-89: Engineers pulse faster.
    Steady,
    /// 90+: Engineers pulse faster, and a last stand is available.
    Unbroken,
}

impl ResolveTier {
    pub fn of(resolve: i32) -> Self {
        if resolve >= RESOLVE_UNBROKEN {
            ResolveTier::Unbroken
        } else if resolve >= RESOLVE_QUICK {
            ResolveTier::Steady
        } else if resolve > RESOLVE_FRAYED {
            ResolveTier::Holding
        } else {
            ResolveTier::Frayed
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ResolveTier::Frayed => "Frayed",
            ResolveTier::Holding => "Holding",
            ResolveTier::Steady => "Steady",
            ResolveTier::Unbroken => "NON DEFICIMUS",
        }
    }
    /// A few words on what the tier is doing for (or to) the crew.
    pub fn effect(self) -> &'static str {
        match self {
            ResolveTier::Frayed => "training +25%",
            ResolveTier::Holding => "",
            ResolveTier::Steady | ResolveTier::Unbroken => "quick hands",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tile {
    Ground,
    Rock,
    Glass(u16),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Order {
    Idle,
    Move((i32, i32)),
    Harvest,
    /// Focus fire on the Hollow with this id.
    Attack(u32),
    /// Advance to the tile, engaging anything met on the way.
    AttackMove((i32, i32)),
    /// Stay put and shoot.
    Hold,
}

#[derive(Debug)]
pub struct Unit {
    pub kind: Kind,
    pub pos: (i32, i32),
    pub hp: i32,
    pub max_hp: i32,
    pub base_max: i32,
    pub order: Order,
    pub selected: bool,
    pub cd: u32,
    pub carry: u16,
    /// Some(ticks left) while the unit is Down and can still be recovered.
    pub down: Option<u32>,
}

#[derive(Debug)]
pub struct Foe {
    pub id: u32,
    pub pos: (i32, i32),
    pub hp: i32,
    pub cd: u32,
}

/// A dead navigation Beacon. Engineers standing next to it can relight it;
/// the Hollow can corrode it back to darkness.
#[derive(Debug)]
pub struct Beacon {
    pub pos: (i32, i32),
    pub lit: bool,
    /// Relight progress in ticks (0..RELIGHT_TICKS) while unlit.
    pub progress: u32,
    /// True once the Driftglass for the current relight has been spent.
    pub paid: bool,
    /// Integrity; only meaningful while lit.
    pub hp: i32,
}

#[derive(Debug)]
pub struct Surge {
    pub in_ticks: u32,
    pub count: u32,
    pub edge: u8,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Playing,
    Won,
    Lost,
}

/// Everything the player can ask of the simulation. Targets are in world
/// coordinates; orders apply to the currently selected crew.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    /// Move to the tile (cadets on Driftglass or the ship resume harvesting).
    Move((i32, i32)),
    /// Focus-fire the Hollow `id` (marines); everyone else moves to `at`.
    Attack {
        id: u32,
        at: (i32, i32),
    },
    /// Marines advance and engage anything they meet; everyone else moves.
    AttackMove((i32, i32)),
    Hold,
    Train(Kind),
    /// Select all able crew inside the inclusive world-coordinate box.
    SelectBox {
        min: (i32, i32),
        max: (i32, i32),
    },
    SelectAll,
    Deselect,
    TogglePause,
}

pub struct Game {
    pub tiles: Vec<Tile>,
    pub units: Vec<Unit>,
    pub foes: Vec<Foe>,
    pub beacons: Vec<Beacon>,
    /// True while the surge in flight is the final one (all Beacons were lit
    /// when it launched). Clearing it with all Beacons still lit wins.
    pub final_surge: bool,
    /// The one-time last stand has already happened.
    pub last_stand_used: bool,
    pub hull: i32,
    pub hull_max: i32,
    pub hull_base: i32,
    pub glass: i32,
    pub wake: f32,
    pub resolve: i32,
    pub tick: u64,
    rng: u64,
    pub surge: Option<Surge>,
    pub surge_live: bool,
    pub surges_launched: u32,
    pub surges_cleared: u32,
    last_surge: u64,
    pub log: VecDeque<String>,
    pub status: Status,
    pub paused: bool,
    next_foe_id: u32,
}

pub fn idx(p: (i32, i32)) -> usize {
    (p.1 * W + p.0) as usize
}

pub fn in_bounds(p: (i32, i32)) -> bool {
    p.0 >= 0 && p.1 >= 0 && p.0 < W && p.1 < H
}

pub fn cheb(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

pub fn in_ship(p: (i32, i32)) -> bool {
    (p.0 - SHIP.0).abs() <= 2 && (p.1 - SHIP.1).abs() <= 1
}

impl Game {
    pub fn new(seed: u64) -> Self {
        let mut g = Game {
            tiles: vec![Tile::Ground; (W * H) as usize],
            units: Vec::new(),
            foes: Vec::new(),
            beacons: Vec::new(),
            final_surge: false,
            last_stand_used: false,
            hull: 200,
            hull_max: 200,
            hull_base: 200,
            glass: 100,
            wake: 0.0,
            resolve: START_RESOLVE,
            tick: 0,
            // Scramble so neighbouring seeds (2k, 2k+1) give different games;
            // xorshift must never start at zero.
            rng: match seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) {
                0 => 1,
                s => s,
            },
            surge: None,
            surge_live: false,
            surges_launched: 0,
            surges_cleared: 0,
            last_surge: 0,
            log: VecDeque::new(),
            status: Status::Playing,
            paused: false,
            next_foe_id: 1,
        };
        g.generate();
        for (k, dx, dy) in [
            (Kind::Cadet, -3, 0),
            (Kind::Cadet, -3, 1),
            (Kind::Engineer, 3, 0),
            (Kind::Marine, 3, 1),
        ] {
            g.spawn_unit(k, (SHIP.0 + dx, SHIP.1 + dy));
        }
        g.say("Captain's Log: the ship is down and mostly upright. Calling that a win.");
        g.say("Cadets are already mining Driftglass. Engineers keep us patched. Marines keep us alive.");
        g.say("Heads up: every crystal we pull wakes the Hollow. Train with 1 / 2 / 3.");
        g.say("Four dead Beacons out there. Somebody should fix that. Probably an Engineer.");
        g
    }

    fn rand(&mut self, n: u32) -> u32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 33) % n as u64) as u32
    }

    fn say(&mut self, s: impl Into<String>) {
        self.log.push_back(s.into());
        while self.log.len() > 60 {
            self.log.pop_front();
        }
    }

    fn generate(&mut self) {
        for y in 0..H {
            for x in 0..W {
                if cheb((x, y), SHIP) > 6 && self.rand(100) < 7 {
                    self.tiles[idx((x, y))] = Tile::Rock;
                }
            }
        }
        let mut centers = vec![(SHIP.0 + 10, SHIP.1 - 4), (SHIP.0 - 11, SHIP.1 + 5)];
        while centers.len() < 14 {
            let c = (
                3 + self.rand((W - 6) as u32) as i32,
                3 + self.rand((H - 6) as u32) as i32,
            );
            if cheb(c, SHIP) >= 10 {
                centers.push(c);
            }
        }
        for c in centers {
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let p = (c.0 + dx, c.1 + dy);
                    if in_bounds(p) && dx * dx + dy * dy <= 5 {
                        self.tiles[idx(p)] = Tile::Glass(40);
                    }
                }
            }
        }
        self.place_beacons();
    }

    /// One Beacon per distance band, on bare ground (never rock or glass)
    /// and reachable on foot from the ship.
    fn place_beacons(&mut self) {
        let start = (SHIP.0 + 3, SHIP.1);
        for (lo, hi) in BEACON_BANDS {
            loop {
                let p = (
                    2 + self.rand((W - 4) as u32) as i32,
                    2 + self.rand((H - 4) as u32) as i32,
                );
                let d = cheb(p, SHIP);
                if d < lo
                    || d > hi
                    || self.tiles[idx(p)] != Tile::Ground
                    || self.step_toward(start, p, 1).is_none()
                {
                    continue;
                }
                self.beacons.push(Beacon {
                    pos: p,
                    lit: false,
                    progress: 0,
                    paid: false,
                    hp: 0,
                });
                break;
            }
        }
    }

    pub fn beacons_lit(&self) -> usize {
        self.beacons.iter().filter(|b| b.lit).count()
    }

    fn walkable(&self, p: (i32, i32)) -> bool {
        in_bounds(p)
            && self.tiles[idx(p)] != Tile::Rock
            && !in_ship(p)
            && !self.beacons.iter().any(|b| b.pos == p)
    }

    /// If `t` is a Beacon (which can't be stood on), the walkable tile next
    /// to it that is closest to `from`; otherwise `t` itself.
    pub(crate) fn standing_spot(&self, t: (i32, i32), from: (i32, i32)) -> (i32, i32) {
        if !self.beacons.iter().any(|b| b.pos == t) {
            return t;
        }
        let mut best: Option<((i32, i32), i32)> = None;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let n = (t.0 + dx, t.1 + dy);
                if (dx, dy) != (0, 0) && self.walkable(n) {
                    let d = cheb(from, n);
                    if best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((n, d));
                    }
                }
            }
        }
        best.map_or(t, |(p, _)| p)
    }

    /// BFS to the first tile within `range` of `to`; returns the next step.
    pub fn step_toward(&self, from: (i32, i32), to: (i32, i32), range: i32) -> Option<(i32, i32)> {
        if cheb(from, to) <= range {
            return None;
        }
        let mut parent = vec![-1i32; (W * H) as usize];
        let mut queue = VecDeque::new();
        parent[idx(from)] = idx(from) as i32;
        queue.push_back(from);
        while let Some(cur) = queue.pop_front() {
            if cheb(cur, to) <= range {
                let mut c = cur;
                while parent[idx(c)] != idx(from) as i32 {
                    let pi = parent[idx(c)];
                    c = (pi % W, pi / W);
                }
                return Some(c);
            }
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let n = (cur.0 + dx, cur.1 + dy);
                    if (dx, dy) != (0, 0) && self.walkable(n) && parent[idx(n)] < 0 {
                        parent[idx(n)] = idx(cur) as i32;
                        queue.push_back(n);
                    }
                }
            }
        }
        None
    }

    fn nearest_glass(&self, from: (i32, i32)) -> Option<(i32, i32)> {
        let mut best: Option<((i32, i32), i32)> = None;
        for y in 0..H {
            for x in 0..W {
                if let Tile::Glass(_) = self.tiles[idx((x, y))] {
                    let d = cheb(from, (x, y));
                    if best.is_none_or(|(_, bd)| d < bd) {
                        best = Some(((x, y), d));
                    }
                }
            }
        }
        best.map(|(p, _)| p)
    }

    fn nearest_foe(&self, from: (i32, i32), range: i32) -> Option<usize> {
        let mut best: Option<(usize, i32)> = None;
        for (i, f) in self.foes.iter().enumerate() {
            let d = cheb(from, f.pos);
            if d <= range && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn spawn_unit(&mut self, kind: Kind, pos: (i32, i32)) {
        let order = if kind == Kind::Cadet {
            Order::Harvest
        } else {
            Order::Idle
        };
        self.units.push(Unit {
            kind,
            pos,
            hp: kind.hp(),
            max_hp: kind.hp(),
            base_max: kind.hp(),
            order,
            selected: false,
            cd: 0,
            carry: 0,
            down: None,
        });
    }

    pub fn tier(&self) -> ResolveTier {
        ResolveTier::of(self.resolve)
    }

    /// What training `kind` costs right now (more while the crew is Frayed).
    pub fn train_cost(&self, kind: Kind) -> i32 {
        let base = kind.cost();
        if self.tier() == ResolveTier::Frayed {
            // round up so the penalty is never smaller than advertised
            (base * FRAYED_COST_PERCENT + 99) / 100
        } else {
            base
        }
    }

    fn train(&mut self, kind: Kind) {
        if self.status != Status::Playing {
            return;
        }
        if self.units.len() >= MAX_CREW {
            self.say("The ship can't support any more crew.");
            return;
        }
        let cost = self.train_cost(kind);
        if self.glass < cost {
            self.say(format!("Not enough Driftglass for a {}.", kind.name()));
            return;
        }
        // First free spot near the ship. New crew deliberately clump on one
        // side: the balance harness showed a tight group focuses fire far
        // better than units spread around the hull.
        let mut spot = None;
        'search: for r in 3..6 {
            for dy in -r..=r {
                for dx in -r..=r {
                    let p = (SHIP.0 + dx, SHIP.1 + dy);
                    if self.walkable(p) && !self.units.iter().any(|u| u.pos == p) {
                        spot = Some(p);
                        break 'search;
                    }
                }
            }
        }
        if let Some(p) = spot {
            self.glass -= cost;
            self.spawn_unit(kind, p);
            self.say(format!("New {} on deck.", kind.name()));
        }
    }

    // -------------------------------------------------------- commands

    /// The single entry point for player input.
    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Move(t) => self.order_move(t),
            Command::Attack { id, at } => self.order_attack(id, at),
            Command::AttackMove(t) => self.order_attack_move(t),
            Command::Hold => self.hold_selected(),
            Command::Train(kind) => self.train(kind),
            Command::SelectBox { min, max } => self.select_box(min, max),
            Command::SelectAll => self.select_all(),
            Command::Deselect => self.units.iter_mut().for_each(|u| u.selected = false),
            Command::TogglePause => self.paused = !self.paused,
        }
    }

    /// The nearest Hollow within one tile of `t`, if any (read-only query
    /// for input code deciding between a move and an attack).
    pub fn foe_near(&self, t: (i32, i32)) -> Option<u32> {
        self.foes
            .iter()
            .filter(|f| cheb(f.pos, t) <= 1)
            .min_by_key(|f| cheb(f.pos, t))
            .map(|f| f.id)
    }

    fn select_all(&mut self) {
        for u in self.units.iter_mut() {
            u.selected = u.down.is_none();
        }
    }

    fn select_box(&mut self, min: (i32, i32), max: (i32, i32)) {
        for u in self.units.iter_mut() {
            u.selected = u.down.is_none()
                && u.pos.0 >= min.0
                && u.pos.0 <= max.0
                && u.pos.1 >= min.1
                && u.pos.1 <= max.1;
        }
    }

    fn order_move(&mut self, t: (i32, i32)) {
        if !in_bounds(t) {
            return;
        }
        let glass_tile = matches!(self.tiles[idx(t)], Tile::Glass(_));
        let ship_tile = in_ship(t);
        let mut units = std::mem::take(&mut self.units);
        for u in units.iter_mut().filter(|u| u.selected && u.down.is_none()) {
            u.order = match u.kind {
                Kind::Cadet if glass_tile || ship_tile => Order::Harvest,
                // clicking a Beacon means "go stand next to it"
                _ => Order::Move(self.standing_spot(t, u.pos)),
            };
        }
        self.units = units;
    }

    fn order_attack(&mut self, id: u32, at: (i32, i32)) {
        if !in_bounds(at) {
            return;
        }
        let glass_tile = matches!(self.tiles[idx(at)], Tile::Glass(_));
        let ship_tile = in_ship(at);
        for u in self
            .units
            .iter_mut()
            .filter(|u| u.selected && u.down.is_none())
        {
            u.order = match u.kind {
                Kind::Marine => Order::Attack(id),
                Kind::Cadet if glass_tile || ship_tile => Order::Harvest,
                _ => Order::Move(at),
            };
        }
    }

    fn order_attack_move(&mut self, t: (i32, i32)) {
        if !in_bounds(t) {
            return;
        }
        let glass_tile = matches!(self.tiles[idx(t)], Tile::Glass(_));
        let ship_tile = in_ship(t);
        for u in self
            .units
            .iter_mut()
            .filter(|u| u.selected && u.down.is_none())
        {
            u.order = match u.kind {
                Kind::Marine => Order::AttackMove(t),
                Kind::Cadet if glass_tile || ship_tile => Order::Harvest,
                _ => Order::Move(t),
            };
        }
    }

    fn hold_selected(&mut self) {
        for u in self
            .units
            .iter_mut()
            .filter(|u| u.selected && u.down.is_none())
        {
            u.order = Order::Hold;
        }
    }

    pub(crate) fn add_foe(&mut self, pos: (i32, i32), cd: u32) {
        let id = self.next_foe_id;
        self.next_foe_id += 1;
        self.foes.push(Foe {
            id,
            pos,
            hp: 20,
            cd,
        });
    }

    // ---------------------------------------------------------- update

    /// Advance the simulation by one tick (10 Hz).
    pub fn update(&mut self) {
        if self.status != Status::Playing || self.paused {
            return;
        }
        self.tick += 1;
        if self.tick.is_multiple_of(50) {
            // every lit Beacon helps the sector settle faster
            let decay = 1.0 + 0.5 * self.beacons_lit() as f32;
            self.wake = (self.wake - decay).max(0.0);
        }
        self.schedule_surge();
        self.run_surge_timer();
        self.update_crew();
        self.update_engineers();
        self.update_beacons();
        self.update_foes();
        self.update_down();
        self.resolve_wave();
    }

    fn schedule_surge(&mut self) {
        if self.surge.is_some() || self.surge_live {
            return;
        }
        let wake_trigger =
            self.wake >= WAKE_TRIGGER_FIRST + WAKE_TRIGGER_STEP * self.surges_launched as f32;
        let timer_trigger = self.tick - self.last_surge >= SURGE_TIMER_TICKS;
        if wake_trigger || timer_trigger {
            let edge = self.rand(4) as u8;
            let count = surge_size(self.surges_launched, self.wake);
            self.surge = Some(Surge {
                in_ticks: SURGE_WARNING_TICKS,
                count,
                edge,
            });
            self.surges_launched += 1;
            self.last_surge = self.tick;
            self.final_surge = self.beacons_lit() == BEACON_COUNT;
            let dir = ["north", "east", "south", "west"][edge as usize];
            let secs = SURGE_WARNING_TICKS / 10;
            if self.final_surge {
                self.say(format!(
                    "FINAL SURGE from the {dir}. {count} contacts. All four lights are up; hold them."
                ));
            } else {
                self.say(format!(
                    "SURGE inbound from the {dir}. {count} contacts, {secs} seconds. Fortify!"
                ));
            }
        }
    }

    /// Engineers next to a dark Beacon relight it: 30 Driftglass up front,
    /// then ~8 seconds of work.
    fn update_beacons(&mut self) {
        for k in 0..self.beacons.len() {
            if self.beacons[k].lit {
                continue;
            }
            let bp = self.beacons[k].pos;
            let tended = self
                .units
                .iter()
                .any(|u| u.kind == Kind::Engineer && u.down.is_none() && cheb(u.pos, bp) <= 1);
            if !tended {
                continue;
            }
            let n = k + 1;
            if !self.beacons[k].paid {
                if self.glass >= RELIGHT_COST {
                    self.glass -= RELIGHT_COST;
                    self.beacons[k].paid = true;
                    self.say(format!(
                        "Beacon {n}: engineer's cracked the casing. Don't breathe on it."
                    ));
                } else if self.tick.is_multiple_of(100) {
                    self.say(format!(
                        "Beacon {n} needs {RELIGHT_COST} Driftglass. We have less. Mine more."
                    ));
                }
                continue;
            }
            self.beacons[k].progress += 1;
            if self.beacons[k].progress >= RELIGHT_TICKS {
                let b = &mut self.beacons[k];
                b.lit = true;
                b.hp = BEACON_HP;
                b.progress = 0;
                b.paid = false;
                self.resolve += BEACON_RESOLVE;
                let lit = self.beacons_lit();
                self.say(format!(
                    "Beacon {n} is lit. {lit} of {BEACON_COUNT}. Small mercies."
                ));
                if lit == BEACON_COUNT {
                    self.say("That's all four. Whatever is out there just noticed.");
                }
            }
        }
    }

    fn hit_beacon(&mut self, k: usize) {
        let n = k + 1;
        let first = self.beacons[k].hp == BEACON_HP;
        self.beacons[k].hp -= 4;
        if self.beacons[k].hp <= 0 {
            let b = &mut self.beacons[k];
            b.lit = false;
            b.hp = 0;
            b.progress = 0;
            b.paid = false;
            self.resolve -= BEACON_RESOLVE;
            self.say(format!(
                "Beacon {n} has gone dark. The Hollow ate the light."
            ));
        } else if first {
            self.say(format!("The Hollow are chewing on Beacon {n}."));
        }
    }

    fn nearest_lit_beacon(&self, from: (i32, i32), range: i32) -> Option<usize> {
        let mut best: Option<(usize, i32)> = None;
        for (k, b) in self.beacons.iter().enumerate() {
            let d = cheb(from, b.pos);
            if b.lit && d <= range && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((k, d));
            }
        }
        best.map(|(k, _)| k)
    }

    fn run_surge_timer(&mut self) {
        let Some(s) = self.surge.as_mut() else { return };
        if s.in_ticks > 0 {
            s.in_ticks -= 1;
            return;
        }
        let (count, edge) = (s.count, s.edge);
        self.surge = None;
        for _ in 0..count {
            let pos = match edge {
                0 => (self.rand(W as u32) as i32, 0),
                1 => (W - 1, self.rand(H as u32) as i32),
                2 => (self.rand(W as u32) as i32, H - 1),
                _ => (0, self.rand(H as u32) as i32),
            };
            self.add_foe(pos, 0);
        }
        self.surge_live = true;
        self.say("The Hollow are here.");
    }

    fn update_crew(&mut self) {
        let mut units = std::mem::take(&mut self.units);
        let moving = self.tick.is_multiple_of(3);
        for u in units.iter_mut() {
            if u.down.is_some() {
                continue;
            }
            if u.cd > 0 {
                u.cd -= 1;
            }
            let order = u.order;
            match order {
                Order::Move(t) => {
                    if u.pos == t {
                        u.order = Order::Idle;
                    } else if moving {
                        match self.step_toward(u.pos, t, 0) {
                            Some(p) => u.pos = p,
                            None => u.order = Order::Idle,
                        }
                    }
                }
                Order::Harvest => self.harvest(u, moving),
                Order::Attack(id) => match self.foes.iter().position(|f| f.id == id) {
                    None => u.order = Order::Idle,
                    Some(i) => {
                        let fp = self.foes[i].pos;
                        if moving
                            && cheb(u.pos, fp) > 3
                            && let Some(p) = self.step_toward(u.pos, fp, 3)
                        {
                            u.pos = p;
                        }
                    }
                },
                Order::AttackMove(t) => {
                    if let Some(i) = self.nearest_foe(u.pos, 6) {
                        let fp = self.foes[i].pos;
                        if moving
                            && cheb(u.pos, fp) > 3
                            && let Some(p) = self.step_toward(u.pos, fp, 3)
                        {
                            u.pos = p;
                        }
                    } else if u.pos == t {
                        u.order = Order::Idle;
                    } else if moving {
                        match self.step_toward(u.pos, t, 0) {
                            Some(p) => u.pos = p,
                            None => u.order = Order::Idle,
                        }
                    }
                }
                Order::Hold | Order::Idle => {}
            }
            if u.kind == Kind::Marine && u.cd == 0 {
                let focus = match u.order {
                    Order::Attack(id) => self.foes.iter().position(|f| f.id == id),
                    _ => None,
                };
                let target = match focus {
                    Some(i) if cheb(u.pos, self.foes[i].pos) <= 4 => Some(i),
                    _ => self.nearest_foe(u.pos, 4),
                };
                if let Some(i) = target {
                    self.foes[i].hp -= 6;
                    u.cd = 5;
                }
            }
        }
        self.units = units;
    }

    fn harvest(&mut self, u: &mut Unit, moving: bool) {
        if u.kind != Kind::Cadet {
            u.order = Order::Idle;
            return;
        }
        if u.carry >= 10 {
            if cheb(u.pos, SHIP) <= 3 {
                self.glass += u.carry as i32;
                self.wake = (self.wake + u.carry as f32 * WAKE_PER_GLASS).min(100.0);
                u.carry = 0;
            } else if moving && let Some(p) = self.step_toward(u.pos, SHIP, 3) {
                u.pos = p;
            }
            return;
        }
        if let Tile::Glass(n) = self.tiles[idx(u.pos)] {
            if self.tick.is_multiple_of(5) {
                u.carry += 1;
                self.tiles[idx(u.pos)] = if n <= 1 {
                    Tile::Ground
                } else {
                    Tile::Glass(n - 1)
                };
            }
            return;
        }
        if moving
            && let Some(t) = self.nearest_glass(u.pos)
            && let Some(p) = self.step_toward(u.pos, t, 0)
        {
            u.pos = p;
        }
    }

    fn update_engineers(&mut self) {
        let pulse = match self.tier() {
            ResolveTier::Steady | ResolveTier::Unbroken => ENGINEER_PULSE_QUICK_TICKS,
            _ => ENGINEER_PULSE_TICKS,
        };
        if !self.tick.is_multiple_of(pulse) {
            return;
        }
        let engineers: Vec<(i32, i32)> = self
            .units
            .iter()
            .filter(|u| u.kind == Kind::Engineer && u.down.is_none())
            .map(|u| u.pos)
            .collect();
        for ep in engineers {
            // 1. recover a Down crewmate
            if self.glass >= 5
                && let Some(i) = self
                    .units
                    .iter()
                    .position(|u| u.down.is_some() && cheb(u.pos, ep) <= 1)
            {
                let u = &mut self.units[i];
                u.hp = (u.max_hp / 2).max(1);
                u.down = None;
                let name = u.kind.name();
                self.glass -= 5;
                self.resolve += 5;
                self.say(format!("Recovered the {name}. We do not fail."));
                continue;
            }
            if self.glass < 1 {
                continue;
            }
            // 2. mend the first worn crewmate nearby
            if let Some(i) = self.units.iter().position(|u| {
                u.down.is_none()
                    && cheb(u.pos, ep) <= 2
                    && (u.hp < u.max_hp || u.max_hp < u.base_max)
            }) {
                let u = &mut self.units[i];
                if u.hp < u.max_hp {
                    u.hp = (u.hp + 4).min(u.max_hp);
                } else {
                    u.max_hp += 1;
                }
                self.glass -= 1;
                continue;
            }
            // 3. patch the hull
            if cheb(ep, SHIP) <= 4 {
                if self.hull < self.hull_max {
                    self.hull = (self.hull + 4).min(self.hull_max);
                    self.glass -= 1;
                } else if self.hull_max < self.hull_base {
                    self.hull_max += 1;
                    self.glass -= 1;
                }
            }
        }
    }

    fn update_foes(&mut self) {
        let moving = self.tick.is_multiple_of(4);
        for i in 0..self.foes.len() {
            if self.foes[i].cd > 0 {
                self.foes[i].cd -= 1;
            }
            let p = self.foes[i].pos;
            let mut best: Option<(usize, i32)> = None;
            for (j, u) in self.units.iter().enumerate() {
                if u.down.is_some() {
                    continue;
                }
                let d = cheb(p, u.pos);
                if d <= 8 && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((j, d));
                }
            }
            match best {
                Some((j, d)) => {
                    if d <= 1 {
                        if self.foes[i].cd == 0 {
                            self.hit_unit(j);
                            self.foes[i].cd = 6;
                        }
                    } else if moving {
                        let target = self.units[j].pos;
                        if let Some(np) = self.step_toward(p, target, 1) {
                            self.foes[i].pos = np;
                        }
                    }
                }
                None => {
                    if cheb(p, SHIP) <= 3 {
                        if self.foes[i].cd == 0 {
                            self.hull -= 4;
                            self.hull_max = (self.hull_max - 1).max(self.hull_base / 2);
                            self.hull = self.hull.min(self.hull_max);
                            self.foes[i].cd = 6;
                        }
                    } else if let Some(k) = self.nearest_lit_beacon(p, 8) {
                        // no crew nearby: corrode a lit Beacon instead
                        let bp = self.beacons[k].pos;
                        if cheb(p, bp) <= 1 {
                            if self.foes[i].cd == 0 {
                                self.hit_beacon(k);
                                self.foes[i].cd = 6;
                            }
                        } else if moving && let Some(np) = self.step_toward(p, bp, 1) {
                            self.foes[i].pos = np;
                        }
                    } else if moving && let Some(np) = self.step_toward(p, SHIP, 3) {
                        self.foes[i].pos = np;
                    }
                }
            }
        }
    }

    fn hit_unit(&mut self, j: usize) {
        let u = &mut self.units[j];
        u.hp -= 4;
        u.max_hp = (u.max_hp - 1).max(5);
        if u.hp <= 0 {
            u.hp = 0;
            u.down = Some(300);
            u.order = Order::Idle;
            u.selected = false;
            let name = u.kind.name();
            let a = if name.starts_with(['a', 'e', 'i', 'o', 'u']) {
                "An"
            } else {
                "A"
            };
            self.say(format!("{a} {name} is down! Get an engineer over there."));
        }
    }

    fn update_down(&mut self) {
        let foe_pos: Vec<(i32, i32)> = self.foes.iter().map(|f| f.pos).collect();
        let mut reclaimed = Vec::new();
        for (i, u) in self.units.iter_mut().enumerate() {
            if let Some(t) = u.down.as_mut() {
                *t = t.saturating_sub(1);
                if *t == 0 || foe_pos.iter().any(|&p| cheb(p, u.pos) <= 1) {
                    reclaimed.push(i);
                }
            }
        }
        for &i in reclaimed.iter().rev() {
            let u = self.units.remove(i);
            self.add_foe(u.pos, 6);
            self.resolve -= RESOLVE_LOST_UNIT;
            self.say(format!(
                "We lost the {}. The Hollow took it.",
                u.kind.name()
            ));
        }
    }

    /// One time per game: a crew that is Unbroken but watching the hull
    /// give out finds one more push. Heals everyone still standing and
    /// shores up the hull, at the cost of most of that Resolve.
    fn maybe_last_stand(&mut self) {
        if self.last_stand_used
            || self.resolve < LAST_STAND_MIN_RESOLVE
            || self.hull <= 0
            || self.hull * LAST_STAND_HULL_FRACTION >= self.hull_base
        {
            return;
        }
        self.last_stand_used = true;
        for u in self.units.iter_mut().filter(|u| u.down.is_none()) {
            u.hp = u.max_hp;
        }
        self.hull += LAST_STAND_HULL_BONUS;
        self.hull_max = self.hull_max.max(self.hull);
        self.resolve = LAST_STAND_RESOLVE;
        self.say("Captain's Log: nobody said 'fall back'. Everyone up. This is the last stand.");
    }

    fn resolve_wave(&mut self) {
        self.foes.retain(|f| f.hp > 0);
        if self.surge_live && self.foes.is_empty() {
            self.surge_live = false;
            self.surges_cleared += 1;
            self.resolve += 10;
            let n = self.surges_cleared;
            self.say(format!("Surge {n} cleared. Crew morale ticks up."));
            if self.final_surge {
                self.final_surge = false;
                if self.beacons_lit() == BEACON_COUNT {
                    self.status = Status::Won;
                    self.say("Captain's Log: four lights, one lane, still here. We do not fail.");
                } else {
                    self.say("We held, but a Beacon went dark on the way. Relight it and wait for the next one.");
                }
            }
        }
        self.resolve = self.resolve.min(100);
        self.maybe_last_stand();
        if self.hull <= 0 || self.resolve <= 0 {
            self.status = Status::Lost;
            self.say("Captain's Log: ...we failed. Not today. Not like this. (press q)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_runs_headless_without_panicking() {
        let mut g = Game::new(42);
        g.apply(Command::Train(Kind::Marine));
        g.apply(Command::Train(Kind::Engineer));
        g.apply(Command::SelectAll);
        for _ in 0..6000 {
            g.update();
            if g.status != Status::Playing {
                break;
            }
        }
        // the economy ran and a surge was at least scheduled
        assert!(g.tick > 0);
        assert!(g.surges_launched >= 1, "timer should force a surge");
    }

    #[test]
    fn cadets_deliver_driftglass_and_raise_wake() {
        let mut g = Game::new(7);
        let start = g.glass;
        for _ in 0..1500 {
            g.update();
        }
        assert!(
            g.glass > start || g.wake > 0.0,
            "harvest loop should pay out"
        );
    }

    #[test]
    fn pathing_routes_around_rock() {
        let g = Game::new(1);
        let step = g.step_toward((SHIP.0 - 10, SHIP.1), (SHIP.0 - 20, SHIP.1), 0);
        assert!(step.is_some());
    }

    fn marine_fixture() -> Game {
        let mut g = Game::new(5);
        g.units.clear();
        g.spawn_unit(Kind::Marine, (SHIP.0 + 8, SHIP.1 + 8));
        g.units[0].selected = true;
        g
    }

    #[test]
    fn right_click_on_hollow_sets_focus_fire() {
        let mut g = marine_fixture();
        let at = (SHIP.0 + 20, SHIP.1 + 8);
        g.add_foe(at, 0);
        let id = g.foes[0].id;
        assert_eq!(g.foe_near(at), Some(id));
        g.apply(Command::Attack { id, at });
        assert!(g.units[0].order == Order::Attack(id));
        // ground click is a plain move
        g.apply(Command::Move((SHIP.0 + 8, SHIP.1 + 20)));
        assert!(g.units[0].order == Order::Move((SHIP.0 + 8, SHIP.1 + 20)));
    }

    #[test]
    fn marines_hit_focus_target_and_unique_ids() {
        let mut g = marine_fixture();
        let pos = g.units[0].pos;
        g.add_foe((pos.0 + 3, pos.1), 99);
        g.add_foe((pos.0 + 2, pos.1 + 1), 99);
        assert!(g.foes[0].id != g.foes[1].id);
        let focus = g.foes[0].id;
        g.units[0].order = Order::Attack(focus);
        g.update();
        assert!(g.foes[0].hp < 20, "focus target should take the shot");
        assert_eq!(g.foes[1].hp, 20);
    }

    #[test]
    fn hold_stays_put_and_attack_move_advances() {
        let mut g = marine_fixture();
        let start = g.units[0].pos;
        g.apply(Command::Hold);
        for _ in 0..30 {
            g.update();
        }
        assert_eq!(g.units[0].pos, start);
        g.apply(Command::AttackMove((start.0, start.1 - 6)));
        for _ in 0..60 {
            g.update();
        }
        assert!(g.units[0].pos != start);
    }

    #[test]
    fn deterministic_for_same_seed() {
        let run = |seed| {
            let mut g = Game::new(seed);
            for _ in 0..2000 {
                g.update();
            }
            (g.glass, g.foes.len(), g.units.len(), g.hull)
        };
        assert_eq!(run(99), run(99));
    }

    #[test]
    fn commands_select_and_order_the_crew() {
        let mut g = Game::new(3);
        g.apply(Command::Train(Kind::Marine));
        // a box around the whole map selects every able unit
        g.apply(Command::SelectBox {
            min: (0, 0),
            max: (W - 1, H - 1),
        });
        assert!(g.units.iter().all(|u| u.selected));
        g.apply(Command::Deselect);
        assert!(g.units.iter().all(|u| !u.selected));
        g.apply(Command::SelectAll);
        // out-of-bounds orders are ignored
        g.apply(Command::Move((-5, 3)));
        assert!(g.units.iter().all(|u| !matches!(u.order, Order::Move(_))));
        // cadets keep harvesting on glass, others move
        g.apply(Command::Move((SHIP.0 + 10, SHIP.1 - 4)));
        for u in &g.units {
            match u.kind {
                Kind::Cadet => assert!(u.order == Order::Harvest),
                _ => assert!(u.order == Order::Move((SHIP.0 + 10, SHIP.1 - 4))),
            }
        }
        // pause freezes the clock
        g.apply(Command::TogglePause);
        let t = g.tick;
        g.update();
        assert_eq!(g.tick, t);
        g.apply(Command::TogglePause);
        g.update();
        assert_eq!(g.tick, t + 1);
    }

    // ------------------------------------------------ resolve tiers

    #[test]
    fn tiers_follow_the_documented_thresholds() {
        for (resolve, tier) in [
            (100, ResolveTier::Unbroken),
            (90, ResolveTier::Unbroken),
            (89, ResolveTier::Steady),
            (70, ResolveTier::Steady),
            (69, ResolveTier::Holding),
            (31, ResolveTier::Holding),
            (30, ResolveTier::Frayed),
            (0, ResolveTier::Frayed),
        ] {
            assert_eq!(ResolveTier::of(resolve), tier, "resolve {resolve}");
        }
        assert_eq!(
            Game::new(1).tier(),
            ResolveTier::Steady,
            "the crew starts Steady"
        );
    }

    /// An engineer next to a badly hurt marine, with Driftglass to spend.
    /// Returns the marine's hp after `ticks` ticks at the given Resolve.
    fn marine_hp_after(resolve: i32, ticks: u32) -> i32 {
        let mut g = Game::new(8);
        g.units.clear();
        g.glass = 500;
        g.spawn_unit(Kind::Engineer, (SHIP.0 + 4, SHIP.1 + 6));
        g.spawn_unit(Kind::Marine, (SHIP.0 + 5, SHIP.1 + 6));
        g.units[1].hp = 1;
        for _ in 0..ticks {
            g.resolve = resolve; // hold the tier steady
            g.update();
        }
        g.units[1].hp
    }

    #[test]
    fn engineers_pulse_every_15_ticks_at_70_resolve_or_more() {
        // 60 ticks: 3 pulses of +4 normally, 4 pulses at 70+
        assert_eq!(marine_hp_after(69, 60), 1 + 3 * 4);
        assert_eq!(marine_hp_after(70, 60), 1 + 4 * 4);
        assert_eq!(marine_hp_after(95, 60), 1 + 4 * 4);
    }

    #[test]
    fn frayed_crews_pay_a_quarter_more_to_train() {
        let mut g = Game::new(1);
        g.resolve = RESOLVE_FRAYED + 1;
        assert_eq!(g.train_cost(Kind::Marine), 40);
        g.resolve = RESOLVE_FRAYED;
        assert_eq!(g.train_cost(Kind::Cadet), 25);
        assert_eq!(g.train_cost(Kind::Engineer), 44, "35 * 1.25 rounds up");
        assert_eq!(g.train_cost(Kind::Marine), 50);

        // the higher price is what is actually charged, and what is required
        g.glass = 49;
        let crew = g.units.len();
        g.apply(Command::Train(Kind::Marine));
        assert_eq!(g.units.len(), crew, "49 is not enough while Frayed");
        g.glass = 50;
        g.apply(Command::Train(Kind::Marine));
        assert_eq!(g.units.len(), crew + 1);
        assert_eq!(g.glass, 0);
    }

    fn last_stand_fixture(hull: i32, resolve: i32) -> Game {
        let mut g = Game::new(6);
        g.hull = hull;
        g.resolve = resolve;
        for u in g.units.iter_mut() {
            u.hp = 1;
        }
        g
    }

    #[test]
    fn last_stand_heals_the_crew_shores_up_the_hull_and_costs_resolve() {
        let mut g = last_stand_fixture(40, 95);
        g.units[0].down = Some(100); // a Down crewmate is not healed
        g.units[0].hp = 0;
        g.resolve_wave();
        assert!(g.last_stand_used);
        assert_eq!(g.hull, 90);
        assert_eq!(g.resolve, LAST_STAND_RESOLVE);
        assert_eq!(g.status, Status::Playing);
        for u in g.units.iter().skip(1) {
            assert_eq!(u.hp, u.max_hp);
        }
        assert_eq!(g.units[0].hp, 0);
        assert!(g.log.iter().any(|l| l.contains("last stand")));
    }

    #[test]
    fn last_stand_needs_unbroken_resolve_and_a_hull_under_a_quarter() {
        // hull exactly at 25% (50 of 200) is not low enough
        let mut g = last_stand_fixture(50, 95);
        g.resolve_wave();
        assert!(!g.last_stand_used);
        // 49 is
        let mut g = last_stand_fixture(49, 95);
        g.resolve_wave();
        assert!(g.last_stand_used);
        // resolve 89 is not Unbroken
        let mut g = last_stand_fixture(20, 89);
        g.resolve_wave();
        assert!(!g.last_stand_used);
        // a hull already at zero is too late
        let mut g = last_stand_fixture(0, 95);
        g.resolve_wave();
        assert!(!g.last_stand_used);
        assert_eq!(g.status, Status::Lost);
    }

    #[test]
    fn last_stand_only_happens_once_per_game() {
        let mut g = last_stand_fixture(30, 95);
        g.resolve_wave();
        assert_eq!(g.hull, 80);
        g.hull = 30;
        g.resolve = 95;
        g.resolve_wave();
        assert_eq!(g.hull, 30, "no second last stand");
        assert_eq!(g.resolve, 95);
    }

    // ----------------------------------------------------- beacons

    /// A fresh game with no crew, one Engineer next to Beacon 0, and
    /// plenty of Driftglass.
    fn beacon_fixture() -> Game {
        let mut g = Game::new(11);
        g.units.clear();
        g.glass = 200;
        let bp = g.beacons[0].pos;
        let stand = g.standing_spot(bp, SHIP);
        assert!(cheb(stand, bp) == 1, "Beacon 0 must have a standing spot");
        g.spawn_unit(Kind::Engineer, stand);
        g
    }

    #[test]
    fn beacons_are_placed_on_bare_ground_at_increasing_distance() {
        for seed in 0..40 {
            let g = Game::new(seed);
            assert_eq!(g.beacons.len(), BEACON_COUNT);
            let mut last = 0;
            for b in &g.beacons {
                assert!(
                    g.tiles[idx(b.pos)] == Tile::Ground,
                    "seed {seed}: beacon not on bare ground"
                );
                assert!(!b.lit);
                let d = cheb(b.pos, SHIP);
                assert!(d > last, "seed {seed}: distances must increase");
                last = d;
            }
        }
    }

    #[test]
    fn engineer_relights_a_beacon_over_eight_seconds_for_thirty_glass() {
        let mut g = beacon_fixture();
        let resolve = g.resolve;
        for _ in 0..40 {
            g.update();
        }
        assert!(!g.beacons[0].lit);
        assert!(g.beacons[0].progress > 0 && g.beacons[0].progress < RELIGHT_TICKS);
        assert_eq!(g.glass, 200 - RELIGHT_COST);
        for _ in 0..60 {
            g.update();
        }
        assert!(g.beacons[0].lit);
        assert_eq!(g.beacons[0].hp, BEACON_HP);
        assert_eq!(g.glass, 200 - RELIGHT_COST, "relight costs 30 once");
        assert_eq!(g.resolve, resolve + BEACON_RESOLVE);
        assert_eq!(g.beacons_lit(), 1);
    }

    #[test]
    fn relight_needs_an_engineer_and_enough_driftglass() {
        // no engineer nearby: nothing happens
        let mut g = beacon_fixture();
        g.units.clear();
        for _ in 0..120 {
            g.update();
        }
        assert!(!g.beacons[0].lit && g.beacons[0].progress == 0 && g.glass == 200);
        // engineer but too little Driftglass: work does not start
        let mut g = beacon_fixture();
        g.glass = RELIGHT_COST - 1;
        for _ in 0..120 {
            g.update();
        }
        assert!(!g.beacons[0].lit && g.beacons[0].progress == 0);
        assert_eq!(g.glass, RELIGHT_COST - 1);
    }

    #[test]
    fn ordering_an_engineer_onto_a_beacon_walks_to_its_side() {
        let mut g = Game::new(11);
        g.units.clear();
        g.spawn_unit(Kind::Engineer, (SHIP.0 + 3, SHIP.1));
        g.units[0].selected = true;
        let bp = g.beacons[0].pos;
        g.apply(Command::Move(bp));
        let Order::Move(dest) = g.units[0].order else {
            panic!("expected a move order")
        };
        assert!(dest != bp && cheb(dest, bp) == 1);
    }

    #[test]
    fn lit_beacons_speed_up_wake_decay() {
        let decayed = |lit: usize| {
            let mut g = Game::new(2);
            g.units.clear();
            g.wake = 50.0;
            g.tick = 49;
            for b in g.beacons.iter_mut().take(lit) {
                b.lit = true;
                b.hp = BEACON_HP;
            }
            g.update();
            50.0 - g.wake
        };
        assert_eq!(decayed(0), 1.0);
        assert_eq!(decayed(2), 2.0);
        assert!(decayed(4) > decayed(1));
    }

    #[test]
    fn hollow_corrode_a_lit_beacon_until_it_goes_dark() {
        let mut g = Game::new(11);
        g.units.clear();
        g.beacons[0].lit = true;
        g.beacons[0].hp = BEACON_HP;
        let resolve = g.resolve;
        let spot = g.standing_spot(g.beacons[0].pos, SHIP);
        g.add_foe(spot, 0);
        let mut chewed = false;
        for _ in 0..400 {
            g.update();
            chewed |= g.beacons[0].hp < BEACON_HP;
            if !g.beacons[0].lit {
                break;
            }
        }
        assert!(chewed, "the Hollow should damage the Beacon first");
        assert!(!g.beacons[0].lit, "a destroyed Beacon goes dark again");
        assert_eq!(g.resolve, resolve - BEACON_RESOLVE);
        assert!(g.log.iter().any(|l| l.contains("gone dark")));
    }

    #[test]
    fn hollow_ignore_unlit_beacons() {
        let mut g = Game::new(11);
        g.units.clear();
        let spot = g.standing_spot(g.beacons[0].pos, SHIP);
        g.add_foe(spot, 0);
        for _ in 0..200 {
            g.update();
        }
        assert_eq!(g.beacons[0].hp, 0);
        assert!(!g.log.iter().any(|l| l.contains("chewing")));
    }

    #[test]
    fn final_surge_is_the_first_surge_after_all_beacons_are_lit() {
        // beacons not all lit: an ordinary surge
        let mut g = Game::new(4);
        g.units.clear();
        g.tick = SURGE_TIMER_TICKS;
        g.schedule_surge();
        assert!(g.surge.is_some() && !g.final_surge);

        // all lit: the next surge is the final one
        let mut g = Game::new(4);
        g.units.clear();
        for b in g.beacons.iter_mut() {
            b.lit = true;
            b.hp = BEACON_HP;
        }
        g.tick = SURGE_TIMER_TICKS;
        g.schedule_surge();
        assert!(g.surge.is_some() && g.final_surge);
        assert!(g.log.iter().any(|l| l.contains("FINAL SURGE")));
    }

    #[test]
    fn win_needs_all_beacons_lit_and_the_final_surge_cleared() {
        // clearing plenty of ordinary surges no longer wins
        let mut g = Game::new(4);
        g.units.clear();
        for _ in 0..5 {
            g.surge_live = true;
            g.resolve_wave();
        }
        assert_eq!(g.status, Status::Playing);
        assert_eq!(g.surges_cleared, 5);

        // final surge cleared with every Beacon lit: win
        let mut g = Game::new(4);
        g.units.clear();
        for b in g.beacons.iter_mut() {
            b.lit = true;
            b.hp = BEACON_HP;
        }
        g.final_surge = true;
        g.surge_live = true;
        g.add_foe((0, 0), 0);
        g.resolve_wave();
        assert_eq!(
            g.status,
            Status::Playing,
            "surge is still live while Hollow remain"
        );
        g.foes.clear();
        g.resolve_wave();
        assert_eq!(g.status, Status::Won);

        // final surge cleared but a Beacon went dark: no win, keep going
        let mut g = Game::new(4);
        g.units.clear();
        for b in g.beacons.iter_mut().take(3) {
            b.lit = true;
            b.hp = BEACON_HP;
        }
        g.final_surge = true;
        g.surge_live = true;
        g.resolve_wave();
        assert_eq!(g.status, Status::Playing);
        assert!(!g.final_surge);
    }

    /// Everything that matters about the sim state, as a comparable string.
    fn fingerprint(g: &Game) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}",
            g.tiles,
            g.units,
            g.foes,
            g.beacons,
            g.surge,
            g.hull,
            g.hull_max,
            g.glass,
            g.wake,
            g.resolve,
            g.tick,
            g.rng,
            g.surges_launched,
            g.surges_cleared,
            g.status,
            g.log,
        )
    }

    #[test]
    fn replaying_the_same_commands_gives_identical_state() {
        // (tick at which to apply, command)
        let script: Vec<(u64, Command)> = vec![
            (0, Command::Train(Kind::Marine)),
            (5, Command::Train(Kind::Engineer)),
            (10, Command::SelectAll),
            (11, Command::Hold),
            (
                20,
                Command::SelectBox {
                    min: (SHIP.0 + 2, SHIP.1),
                    max: (SHIP.0 + 8, SHIP.1 + 6),
                },
            ),
            (21, Command::AttackMove((SHIP.0 + 12, SHIP.1 + 4))),
            (200, Command::Move((SHIP.0 - 8, SHIP.1 - 3))),
            (400, Command::TogglePause),
            (401, Command::TogglePause),
            (600, Command::Train(Kind::Cadet)),
            (700, Command::SelectAll),
            (
                701,
                Command::Attack {
                    id: 1,
                    at: (SHIP.0 + 4, SHIP.1 + 4),
                },
            ),
            (1300, Command::Deselect),
        ];
        let replay = |seed: u64| {
            let mut g = Game::new(seed);
            let mut next = 0;
            for t in 0..3000u64 {
                while next < script.len() && script[next].0 == t {
                    g.apply(script[next].1);
                    next += 1;
                }
                g.update();
            }
            fingerprint(&g)
        };
        let a = replay(1234);
        let b = replay(1234);
        assert_eq!(a, b, "same seed + same commands must match exactly");
        assert_ne!(a, replay(4321), "a different seed should diverge");
    }
}
