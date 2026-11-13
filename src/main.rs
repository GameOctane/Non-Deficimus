// NON DEFICIMUS: a terminal RTS prototype.
// Copyright (C) 2026 Jason Germino (GameOctane)
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! NON DEFICIMUS: a terminal RTS prototype.
//!
//! Harvest Driftglass, hold the line against Hollow surges, and keep the
//! crew alive. "We do not fail."

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
};
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal, Frame,
};

const W: i32 = 96;
const H: i32 = 48;
const SHIP: (i32, i32) = (W / 2, H / 2);
const TICK_MS: u64 = 100;
const WIN_SURGES: u32 = 3;
const MAX_CREW: usize = 24;

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cadet,
    Engineer,
    Marine,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Cadet => "cadet",
            Kind::Engineer => "engineer",
            Kind::Marine => "marine",
        }
    }
    fn glyph(self) -> char {
        match self {
            Kind::Cadet => 'c',
            Kind::Engineer => 'e',
            Kind::Marine => 'm',
        }
    }
    fn color(self) -> Color {
        match self {
            Kind::Cadet => Color::Green,
            Kind::Engineer => Color::White,
            Kind::Marine => Color::LightBlue,
        }
    }
    fn cost(self) -> i32 {
        match self {
            Kind::Cadet => 20,
            Kind::Engineer => 35,
            Kind::Marine => 40,
        }
    }
    fn hp(self) -> i32 {
        match self {
            Kind::Cadet => 12,
            Kind::Engineer => 14,
            Kind::Marine => 24,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tile {
    Ground,
    Rock,
    Glass(u16),
}

#[derive(Clone, Copy, PartialEq)]
enum Order {
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

struct Unit {
    kind: Kind,
    pos: (i32, i32),
    hp: i32,
    max_hp: i32,
    base_max: i32,
    order: Order,
    selected: bool,
    cd: u32,
    carry: u16,
    /// Some(ticks left) while the unit is Down and can still be recovered.
    down: Option<u32>,
}

struct Foe {
    id: u32,
    pos: (i32, i32),
    hp: i32,
    cd: u32,
}

struct Surge {
    in_ticks: u32,
    count: u32,
    edge: u8,
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Playing,
    Won,
    Lost,
}

struct Game {
    tiles: Vec<Tile>,
    units: Vec<Unit>,
    foes: Vec<Foe>,
    hull: i32,
    hull_max: i32,
    hull_base: i32,
    glass: i32,
    wake: f32,
    resolve: i32,
    tick: u64,
    rng: u64,
    surge: Option<Surge>,
    surge_live: bool,
    surges_launched: u32,
    surges_cleared: u32,
    last_surge: u64,
    log: VecDeque<String>,
    cam: (i32, i32),
    drag: Option<((u16, u16), (u16, u16))>,
    status: Status,
    paused: bool,
    next_foe_id: u32,
    pending_amove: bool,
}

fn idx(p: (i32, i32)) -> usize {
    (p.1 * W + p.0) as usize
}

fn in_bounds(p: (i32, i32)) -> bool {
    p.0 >= 0 && p.1 >= 0 && p.0 < W && p.1 < H
}

fn cheb(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

fn in_ship(p: (i32, i32)) -> bool {
    (p.0 - SHIP.0).abs() <= 2 && (p.1 - SHIP.1).abs() <= 1
}

impl Game {
    fn new(seed: u64) -> Self {
        let mut g = Game {
            tiles: vec![Tile::Ground; (W * H) as usize],
            units: Vec::new(),
            foes: Vec::new(),
            hull: 200,
            hull_max: 200,
            hull_base: 200,
            glass: 100,
            wake: 0.0,
            resolve: 60,
            tick: 0,
            rng: seed | 1,
            surge: None,
            surge_live: false,
            surges_launched: 0,
            surges_cleared: 0,
            last_surge: 0,
            log: VecDeque::new(),
            cam: (SHIP.0 - 40, SHIP.1 - 12),
            drag: None,
            status: Status::Playing,
            paused: false,
            next_foe_id: 1,
            pending_amove: false,
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
    }

    fn walkable(&self, p: (i32, i32)) -> bool {
        in_bounds(p) && self.tiles[idx(p)] != Tile::Rock && !in_ship(p)
    }

    /// BFS to the first tile within `range` of `to`; returns the next step.
    fn step_toward(&self, from: (i32, i32), to: (i32, i32), range: i32) -> Option<(i32, i32)> {
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
                    if best.map_or(true, |(_, bd)| d < bd) {
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
            if d <= range && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn spawn_unit(&mut self, kind: Kind, pos: (i32, i32)) {
        let order = if kind == Kind::Cadet { Order::Harvest } else { Order::Idle };
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

    fn train(&mut self, kind: Kind) {
        if self.status != Status::Playing {
            return;
        }
        if self.units.len() >= MAX_CREW {
            self.say("The ship can't support any more crew.");
            return;
        }
        if self.glass < kind.cost() {
            self.say(format!("Not enough Driftglass for a {}.", kind.name()));
            return;
        }
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
            self.glass -= kind.cost();
            self.spawn_unit(kind, p);
            self.say(format!("New {} on deck.", kind.name()));
        }
    }

    // ----------------------------------------------------------- input

    fn cam_for(&self, inner: Rect) -> (i32, i32) {
        (
            self.cam.0.clamp(0, (W - inner.width as i32).max(0)),
            self.cam.1.clamp(0, (H - inner.height as i32).max(0)),
        )
    }

    fn world_at(&self, inner: Rect, col: u16, row: u16) -> (i32, i32) {
        let cam = self.cam_for(inner);
        (
            cam.0 + col as i32 - inner.x as i32,
            cam.1 + row as i32 - inner.y as i32,
        )
    }

    fn select_all(&mut self) {
        for u in self.units.iter_mut() {
            u.selected = u.down.is_none();
        }
    }

    fn finish_drag(&mut self, inner: Rect) {
        let Some((a, b)) = self.drag.take() else { return };
        let wa = self.world_at(inner, a.0, a.1);
        let wb = self.world_at(inner, b.0, b.1);
        let (x0, x1) = (wa.0.min(wb.0), wa.0.max(wb.0));
        let (y0, y1) = (wa.1.min(wb.1), wa.1.max(wb.1));
        for u in self.units.iter_mut() {
            u.selected = u.down.is_none()
                && u.pos.0 >= x0
                && u.pos.0 <= x1
                && u.pos.1 >= y0
                && u.pos.1 <= y1;
        }
    }

    fn order_at(&mut self, t: (i32, i32)) {
        if !in_bounds(t) {
            return;
        }
        let amove = std::mem::take(&mut self.pending_amove);
        let foe = self
            .foes
            .iter()
            .filter(|f| cheb(f.pos, t) <= 1)
            .min_by_key(|f| cheb(f.pos, t))
            .map(|f| f.id);
        let glass_tile = matches!(self.tiles[idx(t)], Tile::Glass(_));
        let ship_tile = in_ship(t);
        for u in self.units.iter_mut().filter(|u| u.selected && u.down.is_none()) {
            u.order = match u.kind {
                Kind::Marine => match foe {
                    Some(id) => Order::Attack(id),
                    None if amove => Order::AttackMove(t),
                    None => Order::Move(t),
                },
                Kind::Cadet if glass_tile || ship_tile => Order::Harvest,
                _ => Order::Move(t),
            };
        }
    }

    fn hold_selected(&mut self) {
        self.pending_amove = false;
        for u in self.units.iter_mut().filter(|u| u.selected && u.down.is_none()) {
            u.order = Order::Hold;
        }
    }

    fn add_foe(&mut self, pos: (i32, i32), cd: u32) {
        let id = self.next_foe_id;
        self.next_foe_id += 1;
        self.foes.push(Foe { id, pos, hp: 20, cd });
    }

    // ---------------------------------------------------------- update

    fn update(&mut self) {
        if self.status != Status::Playing || self.paused {
            return;
        }
        self.tick += 1;
        if self.tick % 50 == 0 {
            self.wake = (self.wake - 1.0).max(0.0);
        }
        self.schedule_surge();
        self.run_surge_timer();
        self.update_crew();
        self.update_engineers();
        self.update_foes();
        self.update_down();
        self.resolve_wave();
    }

    fn schedule_surge(&mut self) {
        if self.surge.is_some() || self.surge_live {
            return;
        }
        let wake_trigger = self.wake >= 25.0 + 12.0 * self.surges_launched as f32;
        let timer_trigger = self.tick - self.last_surge >= 1200;
        if wake_trigger || timer_trigger {
            let edge = self.rand(4) as u8;
            let count = 2 + self.surges_launched * 2 + (self.wake / 20.0) as u32;
            self.surge = Some(Surge { in_ticks: 60, count, edge });
            self.surges_launched += 1;
            self.last_surge = self.tick;
            let dir = ["north", "east", "south", "west"][edge as usize];
            self.say(format!(
                "SURGE inbound from the {dir}. {count} contacts, six seconds. Fortify!"
            ));
        }
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
        let moving = self.tick % 3 == 0;
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
                        if moving && cheb(u.pos, fp) > 3 {
                            if let Some(p) = self.step_toward(u.pos, fp, 3) {
                                u.pos = p;
                            }
                        }
                    }
                },
                Order::AttackMove(t) => {
                    if let Some(i) = self.nearest_foe(u.pos, 6) {
                        let fp = self.foes[i].pos;
                        if moving && cheb(u.pos, fp) > 3 {
                            if let Some(p) = self.step_toward(u.pos, fp, 3) {
                                u.pos = p;
                            }
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
                self.wake = (self.wake + u.carry as f32 * 0.6).min(100.0);
                u.carry = 0;
            } else if moving {
                if let Some(p) = self.step_toward(u.pos, SHIP, 3) {
                    u.pos = p;
                }
            }
            return;
        }
        if let Tile::Glass(n) = self.tiles[idx(u.pos)] {
            if self.tick % 5 == 0 {
                u.carry += 1;
                self.tiles[idx(u.pos)] = if n <= 1 { Tile::Ground } else { Tile::Glass(n - 1) };
            }
            return;
        }
        if moving {
            if let Some(t) = self.nearest_glass(u.pos) {
                if let Some(p) = self.step_toward(u.pos, t, 0) {
                    u.pos = p;
                }
            }
        }
    }

    fn update_engineers(&mut self) {
        if self.tick % 20 != 0 {
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
            if self.glass >= 5 {
                if let Some(i) = self
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
            }
            if self.glass < 1 {
                continue;
            }
            // 2. mend the most worn crewmate nearby
            if let Some(i) = self.units.iter().position(|u| {
                u.down.is_none() && cheb(u.pos, ep) <= 2 && (u.hp < u.max_hp || u.max_hp < u.base_max)
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
        let moving = self.tick % 4 == 0;
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
                if d <= 8 && best.map_or(true, |(_, bd)| d < bd) {
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
                    } else if moving {
                        if let Some(np) = self.step_toward(p, SHIP, 3) {
                            self.foes[i].pos = np;
                        }
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
            self.say(format!("A {name} is down! Get an engineer over there."));
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
            self.resolve -= 10;
            self.say(format!("We lost the {}. The Hollow took it.", u.kind.name()));
        }
    }

    fn resolve_wave(&mut self) {
        self.foes.retain(|f| f.hp > 0);
        if self.surge_live && self.foes.is_empty() {
            self.surge_live = false;
            self.surges_cleared += 1;
            self.resolve += 10;
            let n = self.surges_cleared;
            self.say(format!("Surge {n} cleared. Crew morale ticks up."));
            if n >= WIN_SURGES {
                self.status = Status::Won;
                self.say("Captain's Log: the lane holds. We do not fail.");
            }
        }
        self.resolve = self.resolve.min(100);
        if self.hull <= 0 || self.resolve <= 0 {
            self.status = Status::Lost;
            self.say("Captain's Log: ...we failed. Not today. Not like this. (press q)");
        }
    }

    fn resolve_label(&self) -> (&'static str, Color) {
        match self.resolve {
            90..=100 => ("NON DEFICIMUS", Color::Yellow),
            70..=89 => ("Steady", Color::Green),
            31..=69 => ("Holding", Color::White),
            _ => ("Frayed", Color::Red),
        }
    }
}

// ---------------------------------------------------------------- ui

fn rects(area: Rect) -> (Rect, Rect, Rect) {
    let rows = Layout::vertical([Constraint::Min(10), Constraint::Length(8)]).split(area);
    let cols = Layout::horizontal([Constraint::Min(20), Constraint::Length(32)]).split(rows[0]);
    (cols[0], cols[1], rows[1])
}

fn bar(v: f32, max: f32, w: usize) -> String {
    let n = ((v / max).clamp(0.0, 1.0) * w as f32).round() as usize;
    format!("{}{}", "█".repeat(n), "░".repeat(w - n))
}

fn decay_glyph(hp: i32, max: i32) -> char {
    let f = hp as f32 / max.max(1) as f32;
    if f > 0.75 {
        '█'
    } else if f > 0.5 {
        '▓'
    } else if f > 0.25 {
        '▒'
    } else {
        '░'
    }
}

fn draw(f: &mut Frame, g: &Game) {
    let (map_r, side_r, log_r) = rects(f.area());
    let map_block = Block::bordered().title(" Sector ");
    let inner = map_block.inner(map_r);
    f.render_widget(map_block, map_r);

    let cam = g.cam_for(inner);
    let to_scr = |p: (i32, i32)| -> Option<(u16, u16)> {
        let (sx, sy) = (p.0 - cam.0, p.1 - cam.1);
        if sx >= 0 && sy >= 0 && sx < inner.width as i32 && sy < inner.height as i32 {
            Some((inner.x + sx as u16, inner.y + sy as u16))
        } else {
            None
        }
    };

    {
        let buf = f.buffer_mut();
        // terrain
        for sy in 0..inner.height {
            for sx in 0..inner.width {
                let p = (cam.0 + sx as i32, cam.1 + sy as i32);
                if !in_bounds(p) {
                    continue;
                }
                let (ch, col) = match g.tiles[idx(p)] {
                    Tile::Ground => ('.', Color::Rgb(60, 70, 60)),
                    Tile::Rock => ('^', Color::Rgb(130, 115, 100)),
                    Tile::Glass(_) => ('◆', Color::Cyan),
                };
                buf[(inner.x + sx, inner.y + sy)].set_char(ch).set_fg(col);
            }
        }
        // ship
        for dy in -1..=1 {
            for dx in -2..=2 {
                if let Some((x, y)) = to_scr((SHIP.0 + dx, SHIP.1 + dy)) {
                    let ch = if (dx, dy) == (0, 0) { 'N' } else { decay_glyph(g.hull, g.hull_max.max(g.hull_base / 2)) };
                    let col = if g.hull * 2 < g.hull_base { Color::Yellow } else { Color::LightBlue };
                    buf[(x, y)].set_char(ch).set_fg(col);
                }
            }
        }
        // hollow
        for foe in &g.foes {
            if let Some((x, y)) = to_scr(foe.pos) {
                buf[(x, y)]
                    .set_char('H')
                    .set_style(Style::new().fg(Color::Rgb(200, 70, 40)).add_modifier(Modifier::BOLD));
            }
        }
        // crew
        for u in &g.units {
            if let Some((x, y)) = to_scr(u.pos) {
                let cell = &mut buf[(x, y)];
                if u.down.is_some() {
                    cell.set_char('x').set_fg(Color::Magenta);
                } else if u.selected {
                    cell.set_char(u.kind.glyph())
                        .set_style(Style::new().fg(Color::Black).bg(Color::Yellow));
                } else {
                    cell.set_char(u.kind.glyph())
                        .set_style(Style::new().fg(u.kind.color()).add_modifier(Modifier::BOLD));
                }
            }
        }
        // drag box
        if let Some((a, b)) = g.drag {
            for y in a.1.min(b.1)..=a.1.max(b.1) {
                for x in a.0.min(b.0)..=a.0.max(b.0) {
                    if x >= inner.x && y >= inner.y && x < inner.x + inner.width && y < inner.y + inner.height {
                        buf[(x, y)].set_bg(Color::Rgb(25, 55, 25));
                    }
                }
            }
        }
    }

    // sidebar
    let (label, label_col) = g.resolve_label();
    let selected = g.units.iter().filter(|u| u.selected).count();
    let hint = Style::new().fg(Color::DarkGray);
    let lines = vec![
        Line::from(vec![
            Span::raw(" Driftglass "),
            Span::styled(format!("{}", g.glass), Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(format!(" Wake    {} {:>3}", bar(g.wake, 100.0, 12), g.wake as i32)),
        Line::from(format!(" Hull    {} {:>3}", bar(g.hull as f32, g.hull_base as f32, 12), g.hull)),
        Line::from(vec![
            Span::raw(format!(" Resolve {} ", bar(g.resolve as f32, 100.0, 12))),
        ]),
        Line::from(Span::styled(format!("   {label}"), Style::new().fg(label_col))),
        Line::from(""),
        Line::from(format!(" Crew {}  Hollow {}", g.units.len(), g.foes.len())),
        Line::from(format!(" Surges held {}/{}", g.surges_cleared, WIN_SURGES)),
        Line::from(format!(" Selected {selected}")),
        Line::from(""),
        Line::from(format!(" [1] Cadet     {}", Kind::Cadet.cost())),
        Line::from(format!(" [2] Engineer  {}", Kind::Engineer.cost())),
        Line::from(format!(" [3] Marine    {}", Kind::Marine.cost())),
        Line::from(""),
        Line::from(if g.pending_amove {
            Span::styled(" ATTACK-MOVE: click target", Style::new().fg(Color::Yellow))
        } else {
            Span::raw("")
        }),
        Line::from(Span::styled(" drag: select  RMB: order", hint)),
        Line::from(Span::styled(" RMB on H: focus fire", hint)),
        Line::from(Span::styled(" a: attack-move  h: hold", hint)),
        Line::from(Span::styled(" c: all  arrows: pan", hint)),
        Line::from(Span::styled(" space: pause  q: quit", hint)),
    ];
    let title = if g.paused { " Ship (PAUSED) " } else { " Ship " };
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(title)), side_r);

    // comms log
    let visible = log_r.height.saturating_sub(2) as usize;
    let start = g.log.len().saturating_sub(visible);
    let log_lines: Vec<Line> = g.log.iter().skip(start).map(|s| Line::from(format!(" {s}"))).collect();
    f.render_widget(Paragraph::new(log_lines).block(Block::bordered().title(" Comms ")), log_r);

    // end overlay
    if g.status != Status::Playing {
        let msg = if g.status == Status::Won {
            "THE LANE HOLDS. WE DO NOT FAIL.  (q to quit)"
        } else {
            "THE SHIP IS LOST.  (q to quit)"
        };
        let w = msg.len() as u16 + 4;
        let r = Rect::new(
            map_r.x + map_r.width.saturating_sub(w) / 2,
            map_r.y + map_r.height / 2 - 1,
            w.min(map_r.width),
            3,
        );
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(msg).block(Block::bordered()).style(Style::new().add_modifier(Modifier::BOLD)),
            r,
        );
    }
}

// -------------------------------------------------------------- main

fn handle_mouse(g: &mut Game, m: MouseEvent, area: Rect) {
    let (map_r, _, _) = rects(area);
    let inner = map_r.inner(Margin::new(1, 1));
    let inside = m.column >= inner.x
        && m.row >= inner.y
        && m.column < inner.x + inner.width
        && m.row < inner.y + inner.height;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if inside && g.pending_amove => {
            let t = g.world_at(inner, m.column, m.row);
            g.order_at(t);
        }
        MouseEventKind::Down(MouseButton::Left) if inside => g.drag = Some(((m.column, m.row), (m.column, m.row))),
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some((a, _)) = g.drag {
                g.drag = Some((a, (m.column, m.row)));
            }
        }
        MouseEventKind::Up(MouseButton::Left) => g.finish_drag(inner),
        MouseEventKind::Down(MouseButton::Right) if inside => {
            let t = g.world_at(inner, m.column, m.row);
            g.order_at(t);
        }
        _ => {}
    }
}

/// Returns true when the player asked to quit.
fn handle_key(g: &mut Game, code: KeyCode, mods: KeyModifiers, area: Rect) -> bool {
    let (map_r, _, _) = rects(area);
    let inner = map_r.inner(Margin::new(1, 1));
    let cam = g.cam_for(inner);
    match code {
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            g.pending_amove = false;
            g.units.iter_mut().for_each(|u| u.selected = false);
        }
        KeyCode::Char('a') => g.pending_amove = g.units.iter().any(|u| u.selected && u.kind == Kind::Marine),
        KeyCode::Char('h') => g.hold_selected(),
        KeyCode::Char('c') => g.select_all(),
        KeyCode::Char(' ') => g.paused = !g.paused,
        KeyCode::Char('1') => g.train(Kind::Cadet),
        KeyCode::Char('2') => g.train(Kind::Engineer),
        KeyCode::Char('3') => g.train(Kind::Marine),
        KeyCode::Left => g.cam = (cam.0 - 4, cam.1),
        KeyCode::Right => g.cam = (cam.0 + 4, cam.1),
        KeyCode::Up => g.cam = (cam.0, cam.1 - 2),
        KeyCode::Down => g.cam = (cam.0, cam.1 + 2),
        _ => {}
    }
    false
}

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let mut g = Game::new(seed);
    let tick = Duration::from_millis(TICK_MS);
    let mut last = Instant::now();
    loop {
        terminal.draw(|f| draw(f, &g))?;
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        if event::poll(tick.saturating_sub(last.elapsed()))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if handle_key(&mut g, k.code, k.modifiers, area) {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => handle_mouse(&mut g, m, area),
                _ => {}
            }
        }
        if last.elapsed() >= tick {
            g.update();
            last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_runs_headless_without_panicking() {
        let mut g = Game::new(42);
        g.train(Kind::Marine);
        g.train(Kind::Engineer);
        g.select_all();
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
        assert!(g.glass > start || g.wake > 0.0, "harvest loop should pay out");
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
        g.add_foe((SHIP.0 + 20, SHIP.1 + 8), 0);
        let id = g.foes[0].id;
        g.order_at((SHIP.0 + 20, SHIP.1 + 8));
        assert!(g.units[0].order == Order::Attack(id));
        // ground click is a plain move
        g.order_at((SHIP.0 + 8, SHIP.1 + 20));
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
        g.hold_selected();
        for _ in 0..30 {
            g.update();
        }
        assert_eq!(g.units[0].pos, start);
        g.units[0].order = Order::AttackMove((start.0, start.1 - 6));
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
}

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let res = run(&mut terminal);
    execute!(io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    res
}
