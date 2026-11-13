//! Terminal UI: drawing and input decoding.
//!
//! The UI reads the [`Game`] to render and to look things up, but never
//! mutates it. All player input leaves this module as [`Command`]s.

use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    Frame,
};

use crate::sim::{
    idx, in_bounds, Command, Game, Kind, Status, Tile, BEACON_COUNT, BEACON_HP, H, RELIGHT_TICKS, SHIP, W,
};

/// What a key press asks of the program.
pub enum KeyAction {
    None,
    Quit,
    Command(Command),
}

/// UI-only state: camera, drag box and the pending attack-move prompt.
pub struct Ui {
    pub cam: (i32, i32),
    pub drag: Option<((u16, u16), (u16, u16))>,
    pub pending_amove: bool,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            cam: (SHIP.0 - 40, SHIP.1 - 12),
            drag: None,
            pending_amove: false,
        }
    }

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

    /// Finish a drag-box and turn it into a selection command.
    fn finish_drag(&mut self, inner: Rect) -> Option<Command> {
        let (a, b) = self.drag.take()?;
        let wa = self.world_at(inner, a.0, a.1);
        let wb = self.world_at(inner, b.0, b.1);
        Some(Command::SelectBox {
            min: (wa.0.min(wb.0), wa.1.min(wb.1)),
            max: (wa.0.max(wb.0), wa.1.max(wb.1)),
        })
    }

    /// Turn a right-click (or attack-move click) on world tile `t` into an order.
    fn order_at(&mut self, g: &Game, t: (i32, i32)) -> Option<Command> {
        if !in_bounds(t) {
            return None;
        }
        let amove = std::mem::take(&mut self.pending_amove);
        Some(match g.foe_near(t) {
            Some(id) => Command::Attack { id, at: t },
            None if amove => Command::AttackMove(t),
            None => Command::Move(t),
        })
    }
}

// ---------------------------------------------------------------- draw

fn kind_color(k: Kind) -> Color {
    match k {
        Kind::Cadet => Color::Green,
        Kind::Engineer => Color::White,
        Kind::Marine => Color::LightBlue,
    }
}

fn resolve_label(resolve: i32) -> (&'static str, Color) {
    match resolve {
        90..=100 => ("NON DEFICIMUS", Color::Yellow),
        70..=89 => ("Steady", Color::Green),
        31..=69 => ("Holding", Color::White),
        _ => ("Frayed", Color::Red),
    }
}

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

pub fn draw(f: &mut Frame, g: &Game, ui: &Ui) {
    let (map_r, side_r, log_r) = rects(f.area());
    let map_block = Block::bordered().title(" Sector ");
    let inner = map_block.inner(map_r);
    f.render_widget(map_block, map_r);

    let cam = ui.cam_for(inner);
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
        // beacons: o = dark, o (bold yellow) = being relit, * = lit (orange when corroded)
        for b in &g.beacons {
            if let Some((x, y)) = to_scr(b.pos) {
                let (ch, style) = if b.lit {
                    let col = if b.hp * 2 < BEACON_HP { Color::Rgb(230, 120, 40) } else { Color::LightYellow };
                    ('*', Style::new().fg(col).add_modifier(Modifier::BOLD))
                } else if b.progress > 0 {
                    ('o', Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                } else {
                    ('o', Style::new().fg(Color::Rgb(150, 60, 60)))
                };
                buf[(x, y)].set_char(ch).set_style(style);
                // relight progress: a five-cell bar just above the Beacon
                if !b.lit && b.progress > 0 {
                    let filled = (b.progress * 5 / RELIGHT_TICKS) as i32;
                    for i in 0..5 {
                        let bar_pos = (b.pos.0 - 2 + i, b.pos.1 - 1);
                        if let Some((bx, by)) = to_scr(bar_pos) {
                            let (c, col) = if i < filled { ('█', Color::Yellow) } else { ('░', Color::DarkGray) };
                            buf[(bx, by)].set_char(c).set_fg(col);
                        }
                    }
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
                        .set_style(Style::new().fg(kind_color(u.kind)).add_modifier(Modifier::BOLD));
                }
            }
        }
        // drag box
        if let Some((a, b)) = ui.drag {
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
    let (label, label_col) = resolve_label(g.resolve);
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
        Line::from(format!(" Crew {}  Hollow {}", g.units.len(), g.foes.len())),
        Line::from(format!(" Beacons lit {}/{}", g.beacons_lit(), BEACON_COUNT)),
        Line::from(format!(" Surges held {}", g.surges_cleared)),
        Line::from(format!(" Selected {selected}")),
        Line::from(""),
        Line::from(format!(" [1] Cadet     {}", Kind::Cadet.cost())),
        Line::from(format!(" [2] Engineer  {}", Kind::Engineer.cost())),
        Line::from(format!(" [3] Marine    {}", Kind::Marine.cost())),
        Line::from(""),
        Line::from(if ui.pending_amove {
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

// --------------------------------------------------------------- input

pub fn handle_mouse(ui: &mut Ui, g: &Game, m: MouseEvent, area: Rect) -> Option<Command> {
    let (map_r, _, _) = rects(area);
    let inner = map_r.inner(Margin::new(1, 1));
    let inside = m.column >= inner.x
        && m.row >= inner.y
        && m.column < inner.x + inner.width
        && m.row < inner.y + inner.height;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if inside && ui.pending_amove => {
            let t = ui.world_at(inner, m.column, m.row);
            ui.order_at(g, t)
        }
        MouseEventKind::Down(MouseButton::Left) if inside => {
            ui.drag = Some(((m.column, m.row), (m.column, m.row)));
            None
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some((a, _)) = ui.drag {
                ui.drag = Some((a, (m.column, m.row)));
            }
            None
        }
        MouseEventKind::Up(MouseButton::Left) => ui.finish_drag(inner),
        MouseEventKind::Down(MouseButton::Right) if inside => {
            let t = ui.world_at(inner, m.column, m.row);
            ui.order_at(g, t)
        }
        _ => None,
    }
}

pub fn handle_key(ui: &mut Ui, g: &Game, code: KeyCode, mods: KeyModifiers, area: Rect) -> KeyAction {
    let (map_r, _, _) = rects(area);
    let inner = map_r.inner(Margin::new(1, 1));
    let cam = ui.cam_for(inner);
    match code {
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
        KeyCode::Char('q') => KeyAction::Quit,
        KeyCode::Esc => {
            ui.pending_amove = false;
            KeyAction::Command(Command::Deselect)
        }
        KeyCode::Char('a') => {
            ui.pending_amove = g.units.iter().any(|u| u.selected && u.kind == Kind::Marine);
            KeyAction::None
        }
        KeyCode::Char('h') => {
            ui.pending_amove = false;
            KeyAction::Command(Command::Hold)
        }
        KeyCode::Char('c') => KeyAction::Command(Command::SelectAll),
        KeyCode::Char(' ') => KeyAction::Command(Command::TogglePause),
        KeyCode::Char('1') => KeyAction::Command(Command::Train(Kind::Cadet)),
        KeyCode::Char('2') => KeyAction::Command(Command::Train(Kind::Engineer)),
        KeyCode::Char('3') => KeyAction::Command(Command::Train(Kind::Marine)),
        KeyCode::Left => {
            ui.cam = (cam.0 - 4, cam.1);
            KeyAction::None
        }
        KeyCode::Right => {
            ui.cam = (cam.0 + 4, cam.1);
            KeyAction::None
        }
        KeyCode::Up => {
            ui.cam = (cam.0, cam.1 - 2);
            KeyAction::None
        }
        KeyCode::Down => {
            ui.cam = (cam.0, cam.1 + 2);
            KeyAction::None
        }
        _ => KeyAction::None,
    }
}
