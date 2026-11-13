//! Terminal UI: drawing and input decoding.
//!
//! The UI reads the [`Game`] to render and to look things up, but never
//! mutates it. All player input leaves this module as [`Command`]s.

use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};

use crate::sim::{
    BEACON_COUNT, BEACON_HP, Command, Game, H, Kind, LOG_LINES, RELIGHT_TICKS, ResolveTier, SHIP,
    Status, TURRET_BUILD_TICKS, TURRET_CAP, TURRET_COST, TURRET_HP, Tile, W, idx, in_bounds,
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
    /// Waiting for a click to place a turret (after pressing `4`).
    pub pending_build: bool,
    /// The captain's briefing is on screen (shown at the start, and on `?`).
    pub show_help: bool,
    /// How many (wrapped) comms lines the log is scrolled back; 0 = latest.
    pub log_scroll: usize,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            cam: (SHIP.0 - 40, SHIP.1 - 12),
            drag: None,
            pending_amove: false,
            pending_build: false,
            show_help: true,
            log_scroll: 0,
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

/// How many of each crew type are on their feet, coloured like their map
/// glyphs. A zero Engineer count is shown in red: it is easy to miss when
/// everyone is clumped together.
fn roster_line(g: &Game) -> Line<'static> {
    let count = |k: Kind| g.crew_count(k);
    let engineers = count(Kind::Engineer);
    let eng_style = if engineers == 0 {
        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(kind_color(Kind::Engineer))
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("Cad {}", count(Kind::Cadet)),
            Style::new().fg(kind_color(Kind::Cadet)),
        ),
        Span::raw("  "),
        Span::styled(format!("Eng {engineers}"), eng_style),
        Span::raw("  "),
        Span::styled(
            format!("Mar {}", count(Kind::Marine)),
            Style::new().fg(kind_color(Kind::Marine)),
        ),
    ];
    if g.down_count() > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("Down {}", g.down_count()),
            Style::new().fg(Color::Magenta),
        ));
    }
    Line::from(spans)
}

fn tier_color(t: ResolveTier) -> Color {
    match t {
        ResolveTier::Unbroken => Color::Yellow,
        ResolveTier::Steady => Color::Green,
        ResolveTier::Holding => Color::White,
        ResolveTier::Frayed => Color::Red,
    }
}

/// "Steady: quick hands", or just the name when the tier has no effect.
fn tier_text(t: ResolveTier) -> String {
    match t.effect() {
        "" => t.name().to_string(),
        effect => format!("{}: {effect}", t.name()),
    }
}

/// Where the next surge stands: announced (with its countdown), live, or
/// the latest it can arrive on the timer. Wake can bring it sooner.
fn surge_line(g: &Game) -> Line<'static> {
    let warn = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
    if let Some(s) = &g.surge {
        let tag = if g.final_surge {
            "FINAL SURGE"
        } else {
            "SURGE"
        };
        let secs = s.in_ticks.div_ceil(10);
        Line::from(Span::styled(format!(" {tag} in {secs}s!"), warn))
    } else if g.surge_live {
        let tag = if g.final_surge {
            "FINAL SURGE"
        } else {
            "Surge"
        };
        Line::from(Span::styled(format!(" {tag} is here"), warn))
    } else {
        let secs = g.surge_eta().unwrap_or(0).div_ceil(10);
        let text = format!(" Next surge by {}:{:02}", secs / 60, secs % 60);
        let style = if g.all_lit_since.is_some() {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        Line::from(Span::styled(text, style))
    }
}

/// The opening briefing: the goal, how to get more crew, and the basics.
fn draw_briefing(f: &mut Frame, area: Rect) {
    let head = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let key = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(Color::DarkGray);
    let lines = vec![
        Line::from(Span::styled("\"We do not fail.\"", head)),
        Line::from(""),
        Line::from(
            "The ship is down in a dead sector. Relight all four Beacons (o), then survive the final surge.",
        ),
        Line::from(""),
        Line::from(Span::styled("YOU CAN TRAIN MORE CREW. Press:", head)),
        Line::from(vec![
            Span::styled("  1  ", key),
            Span::raw("Cadet    mines Driftglass (and wakes the Hollow)"),
        ]),
        Line::from(vec![
            Span::styled("  2  ", key),
            Span::raw("Engineer repairs, recovers the Down, relights Beacons"),
        ]),
        Line::from(vec![
            Span::styled("  3  ", key),
            Span::raw("Marine   fights"),
        ]),
        Line::from(vec![
            Span::styled("  4  ", key),
            Span::raw("Turret   then click a tile near the ship; an Engineer builds it"),
        ]),
        Line::from("Crew costs Driftglass. Start with a few Marines: a surge is coming."),
        Line::from(""),
        Line::from(Span::styled("Beacons", head)),
        Line::from("Park an Engineer next to a dark Beacon: 30 Driftglass, about 8 seconds."),
        Line::from(
            "The fourth light calls the FINAL SURGE: 10 or more Hollow. Build a defence first.",
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled("Drag", key),
            Span::raw(" to select.  "),
            Span::styled("Right-click", key),
            Span::raw(" to order.  "),
            Span::styled("Arrows", key),
            Span::raw(" pan.  "),
            Span::styled("c", key),
            Span::raw(" all crew, "),
            Span::styled("e", key),
            Span::raw(" Engineers, "),
            Span::styled("m", key),
            Span::raw(" Marines.  "),
            Span::styled("?", key),
            Span::raw(" shows this again.  "),
            Span::styled("PgUp/PgDn", key),
            Span::raw(" scroll the log."),
        ]),
        Line::from(""),
        Line::from(Span::styled("Press any key to begin.", dim)),
    ];
    let w = area.width.min(74);
    let h = (lines.len() as u16 + 4).min(area.height);
    let r = Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::bordered()
                .title(" Captain's briefing ")
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        r,
    );
}

/// Terminal size the layout is designed for.
const MIN_COLS: u16 = 100;
const MIN_ROWS: u16 = 30;
/// Lines moved per PageUp/PageDown.
const LOG_PAGE: usize = 4;

/// Break one comms message into lines of at most `width` characters,
/// indenting continuation lines.
fn wrap(msg: &str, width: usize) -> Vec<String> {
    let width = width.max(12);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in msg.split_whitespace() {
        let fresh = cur.trim().is_empty();
        if !fresh && cur.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
            cur.push_str("   ");
        } else if !fresh {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.trim().is_empty() {
        lines.push(cur);
    }
    lines
}

fn wrapped_log(g: &Game, width: usize) -> Vec<String> {
    g.log.iter().flat_map(|m| wrap(m, width)).collect()
}

fn rects(area: Rect) -> (Rect, Rect, Rect) {
    // the comms log gets taller as the terminal does (8 to 14 rows)
    let log_h = (area.height / 4).clamp(8, 14);
    let rows = Layout::vertical([Constraint::Min(10), Constraint::Length(log_h)]).split(area);
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
    let area = f.area();
    let title = if area.width < MIN_COLS || area.height < MIN_ROWS {
        format!(
            " Sector (terminal {}x{}; {MIN_COLS}x{MIN_ROWS} recommended) ",
            area.width, area.height
        )
    } else {
        " Sector ".to_string()
    };
    let map_block = Block::bordered().title(title);
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
                    let ch = if (dx, dy) == (0, 0) {
                        'N'
                    } else {
                        decay_glyph(g.hull, g.hull_max.max(g.hull_base / 2))
                    };
                    let col = if g.hull * 2 < g.hull_base {
                        Color::Yellow
                    } else {
                        Color::LightBlue
                    };
                    buf[(x, y)].set_char(ch).set_fg(col);
                }
            }
        }
        // beacons: o = dark, o (bold yellow) = being relit, * = lit (orange when corroded)
        for b in &g.beacons {
            if let Some((x, y)) = to_scr(b.pos) {
                let (ch, style) = if b.lit {
                    let col = if b.hp * 2 < BEACON_HP {
                        Color::Rgb(230, 120, 40)
                    } else {
                        Color::LightYellow
                    };
                    ('*', Style::new().fg(col).add_modifier(Modifier::BOLD))
                } else if b.progress > 0 {
                    (
                        'o',
                        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )
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
                            let (c, col) = if i < filled {
                                ('█', Color::Yellow)
                            } else {
                                ('░', Color::DarkGray)
                            };
                            buf[(bx, by)].set_char(c).set_fg(col);
                        }
                    }
                }
            }
        }
        // turrets: T = built (orange when damaged), t = going up (with a progress bar)
        for t in &g.turrets {
            if let Some((x, y)) = to_scr(t.pos) {
                let (ch, style) = if t.built {
                    let col = if t.hp * 2 < TURRET_HP {
                        Color::Rgb(230, 120, 40)
                    } else {
                        Color::LightGreen
                    };
                    ('T', Style::new().fg(col).add_modifier(Modifier::BOLD))
                } else {
                    (
                        't',
                        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )
                };
                buf[(x, y)].set_char(ch).set_style(style);
                if !t.built && t.progress > 0 {
                    let filled = (t.progress * 5 / TURRET_BUILD_TICKS) as i32;
                    for i in 0..5 {
                        if let Some((bx, by)) = to_scr((t.pos.0 - 2 + i, t.pos.1 - 1)) {
                            let (c, col) = if i < filled {
                                ('█', Color::Yellow)
                            } else {
                                ('░', Color::DarkGray)
                            };
                            buf[(bx, by)].set_char(c).set_fg(col);
                        }
                    }
                }
            }
        }
        // hollow
        for foe in &g.foes {
            if let Some((x, y)) = to_scr(foe.pos) {
                buf[(x, y)].set_char('H').set_style(
                    Style::new()
                        .fg(Color::Rgb(200, 70, 40))
                        .add_modifier(Modifier::BOLD),
                );
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
                    cell.set_char(u.kind.glyph()).set_style(
                        Style::new()
                            .fg(kind_color(u.kind))
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
        // drag box
        if let Some((a, b)) = ui.drag {
            for y in a.1.min(b.1)..=a.1.max(b.1) {
                for x in a.0.min(b.0)..=a.0.max(b.0) {
                    if x >= inner.x
                        && y >= inner.y
                        && x < inner.x + inner.width
                        && y < inner.y + inner.height
                    {
                        buf[(x, y)].set_bg(Color::Rgb(25, 55, 25));
                    }
                }
            }
        }
    }

    // sidebar
    let tier = g.tier();
    let (label, label_col) = (tier_text(tier), tier_color(tier));
    let selected = g.units.iter().filter(|u| u.selected).count();
    let hint = Style::new().fg(Color::DarkGray);
    let lines = vec![
        Line::from(vec![
            Span::raw(" Driftglass "),
            Span::styled(
                format!("{}", g.glass),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!(
            " Wake    {} {:>3}",
            bar(g.wake, 100.0, 12),
            g.wake as i32
        )),
        Line::from(format!(
            " Hull    {} {:>3}",
            bar(g.hull as f32, g.hull_base as f32, 12),
            g.hull
        )),
        Line::from(vec![Span::raw(format!(
            " Resolve {} ",
            bar(g.resolve as f32, 100.0, 12)
        ))]),
        Line::from(Span::styled(
            format!("   {label}"),
            Style::new().fg(label_col),
        )),
        Line::from(format!(
            " Crew {}  Sel {selected}  Hollow {}",
            g.units.len(),
            g.foes.len()
        )),
        roster_line(g),
        Line::from(format!(
            " Beacons {}/{}  Held {}",
            g.beacons_lit(),
            BEACON_COUNT,
            g.surges_cleared
        )),
        surge_line(g),
        Line::from(format!(" [1] Cadet     {}", g.train_cost(Kind::Cadet))),
        Line::from(format!(" [2] Engineer  {}", g.train_cost(Kind::Engineer))),
        Line::from(format!(" [3] Marine    {}", g.train_cost(Kind::Marine))),
        Line::from(format!(
            " [4] Turret    {TURRET_COST}  ({}/{TURRET_CAP})",
            g.turrets.len()
        )),
        Line::from(if ui.pending_build {
            Span::styled(" BUILD: click a tile", Style::new().fg(Color::Yellow))
        } else if ui.pending_amove {
            Span::styled(" ATTACK-MOVE: click target", Style::new().fg(Color::Yellow))
        } else {
            Span::raw("")
        }),
        Line::from(Span::styled(" drag: select  RMB: order", hint)),
        Line::from(Span::styled(" e: all eng.  m: all marines", hint)),
        Line::from(Span::styled(" a: attack-move  h: hold", hint)),
        Line::from(Span::styled(" c: all  arrows: pan", hint)),
        Line::from(Span::styled(" ?: help  spc: pause  q: quit", hint)),
    ];
    let title = if g.paused {
        " Ship (PAUSED) "
    } else {
        " Ship "
    };
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        side_r,
    );

    // comms log
    let visible = log_r.height.saturating_sub(2) as usize;
    let wrapped = wrapped_log(g, log_r.width.saturating_sub(3) as usize);
    let scroll = ui.log_scroll.min(wrapped.len().saturating_sub(visible));
    let end = wrapped.len() - scroll;
    let start = end.saturating_sub(visible);
    let log_lines: Vec<Line> = wrapped[start..end]
        .iter()
        .map(|s| Line::from(format!(" {s}")))
        .collect();
    let log_title = if scroll > 0 {
        " Comms (scrolled back: End = latest) "
    } else {
        " Comms (PgUp/PgDn: history) "
    };
    f.render_widget(
        Paragraph::new(log_lines).block(Block::bordered().title(log_title)),
        log_r,
    );

    if ui.show_help {
        draw_briefing(f, f.area());
    }

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
            map_r.y + (map_r.height / 2).saturating_sub(1),
            w.min(map_r.width),
            3,
        );
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(msg)
                .block(Block::bordered())
                .style(Style::new().add_modifier(Modifier::BOLD)),
            r,
        );
    }
}

// --------------------------------------------------------------- input

pub fn handle_mouse(ui: &mut Ui, g: &Game, m: MouseEvent, area: Rect) -> Option<Command> {
    if ui.show_help {
        // a click closes the briefing and is otherwise ignored
        if matches!(m.kind, MouseEventKind::Down(_)) {
            ui.show_help = false;
        }
        return None;
    }
    let (map_r, _, _) = rects(area);
    let inner = map_r.inner(Margin::new(1, 1));
    let inside = m.column >= inner.x
        && m.row >= inner.y
        && m.column < inner.x + inner.width
        && m.row < inner.y + inner.height;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if inside && ui.pending_build => {
            ui.pending_build = false;
            Some(Command::Build(ui.world_at(inner, m.column, m.row)))
        }
        MouseEventKind::Down(MouseButton::Right) if ui.pending_build => {
            ui.pending_build = false;
            None
        }
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

pub fn handle_key(
    ui: &mut Ui,
    g: &Game,
    code: KeyCode,
    mods: KeyModifiers,
    area: Rect,
) -> KeyAction {
    let (map_r, _, _) = rects(area);
    let inner = map_r.inner(Margin::new(1, 1));
    let cam = ui.cam_for(inner);
    if ui.show_help {
        // any key closes the briefing and is otherwise ignored
        ui.show_help = false;
        let quit = matches!(code, KeyCode::Char('q'))
            || (matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL));
        return if quit {
            KeyAction::Quit
        } else {
            KeyAction::None
        };
    }
    match code {
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
        KeyCode::Char('q') => KeyAction::Quit,
        KeyCode::Char('?') => {
            ui.show_help = true;
            KeyAction::None
        }
        KeyCode::PageUp => {
            // the log never holds more than LOG_LINES messages (a few lines each)
            ui.log_scroll = (ui.log_scroll + LOG_PAGE).min(LOG_LINES * 4);
            KeyAction::None
        }
        KeyCode::PageDown => {
            ui.log_scroll = ui.log_scroll.saturating_sub(LOG_PAGE);
            KeyAction::None
        }
        KeyCode::End => {
            ui.log_scroll = 0;
            KeyAction::None
        }
        KeyCode::Esc => {
            ui.pending_amove = false;
            ui.pending_build = false;
            KeyAction::Command(Command::Deselect)
        }
        KeyCode::Char('a') => {
            ui.pending_build = false;
            ui.pending_amove = g.units.iter().any(|u| u.selected && u.kind == Kind::Marine);
            KeyAction::None
        }
        KeyCode::Char('h') => {
            ui.pending_amove = false;
            KeyAction::Command(Command::Hold)
        }
        KeyCode::Char('c') => KeyAction::Command(Command::SelectAll),
        KeyCode::Char('e') => KeyAction::Command(Command::SelectKind(Kind::Engineer)),
        KeyCode::Char('m') => KeyAction::Command(Command::SelectKind(Kind::Marine)),
        KeyCode::Char(' ') => KeyAction::Command(Command::TogglePause),
        KeyCode::Char('1') => KeyAction::Command(Command::Train(Kind::Cadet)),
        KeyCode::Char('2') => KeyAction::Command(Command::Train(Kind::Engineer)),
        KeyCode::Char('3') => KeyAction::Command(Command::Train(Kind::Marine)),
        KeyCode::Char('4') => {
            ui.pending_amove = false;
            ui.pending_build = true;
            KeyAction::None
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::MouseEvent;

    // 100x30 terminal: the map interior starts at (1, 1) and the default
    // camera sits at (8, 12), so world (wx, wy) is screen (wx - 7, wy - 11).
    fn area() -> Rect {
        Rect::new(0, 0, 100, 30)
    }

    /// A Ui with the opening briefing already dismissed.
    fn quiet_ui() -> Ui {
        let mut ui = Ui::new();
        ui.show_help = false;
        ui
    }

    fn mouse(kind: MouseEventKind, wx: i32, wy: i32) -> MouseEvent {
        MouseEvent {
            kind,
            column: (wx - 7) as u16,
            row: (wy - 11) as u16,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn key(ui: &mut Ui, g: &Game, c: char) -> KeyAction {
        handle_key(ui, g, KeyCode::Char(c), KeyModifiers::NONE, area())
    }

    #[test]
    fn keys_become_commands_and_quit() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        assert!(matches!(
            key(&mut ui, &g, '1'),
            KeyAction::Command(Command::Train(Kind::Cadet))
        ));
        assert!(matches!(
            key(&mut ui, &g, '3'),
            KeyAction::Command(Command::Train(Kind::Marine))
        ));
        assert!(matches!(
            key(&mut ui, &g, ' '),
            KeyAction::Command(Command::TogglePause)
        ));
        assert!(matches!(
            key(&mut ui, &g, 'c'),
            KeyAction::Command(Command::SelectAll)
        ));
        assert!(matches!(key(&mut ui, &g, 'q'), KeyAction::Quit));
        let ctrl_c = handle_key(
            &mut ui,
            &g,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            area(),
        );
        assert!(matches!(ctrl_c, KeyAction::Quit));
    }

    #[test]
    fn arrows_pan_the_camera_without_touching_the_game() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        let before = ui.cam;
        let a = handle_key(&mut ui, &g, KeyCode::Right, KeyModifiers::NONE, area());
        assert!(matches!(a, KeyAction::None));
        assert_eq!(ui.cam, (before.0 + 4, before.1));
    }

    #[test]
    fn attack_move_needs_a_marine_and_clears_on_esc() {
        let (mut ui, mut g) = (quiet_ui(), Game::new(1));
        key(&mut ui, &g, 'a');
        assert!(!ui.pending_amove, "no marine selected, nothing to prompt");
        g.apply(Command::SelectAll);
        key(&mut ui, &g, 'a');
        assert!(ui.pending_amove);
        let esc = handle_key(&mut ui, &g, KeyCode::Esc, KeyModifiers::NONE, area());
        assert!(matches!(esc, KeyAction::Command(Command::Deselect)));
        assert!(!ui.pending_amove);
    }

    #[test]
    fn right_click_issues_move_or_attack() {
        let (mut ui, mut g) = (quiet_ui(), Game::new(1));
        let ground = (SHIP.0 + 6, SHIP.1 + 4);
        let c = handle_mouse(
            &mut ui,
            &g,
            mouse(MouseEventKind::Down(MouseButton::Right), ground.0, ground.1),
            area(),
        );
        assert_eq!(c, Some(Command::Move(ground)));

        let foe_at = (SHIP.0 + 9, SHIP.1 + 6);
        g.add_foe(foe_at, 0);
        let id = g.foes[0].id;
        let c = handle_mouse(
            &mut ui,
            &g,
            mouse(MouseEventKind::Down(MouseButton::Right), foe_at.0, foe_at.1),
            area(),
        );
        assert_eq!(c, Some(Command::Attack { id, at: foe_at }));
    }

    #[test]
    fn pending_attack_move_click_orders_and_clears() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        ui.pending_amove = true;
        let t = (SHIP.0 + 6, SHIP.1 + 4);
        let c = handle_mouse(
            &mut ui,
            &g,
            mouse(MouseEventKind::Down(MouseButton::Left), t.0, t.1),
            area(),
        );
        assert_eq!(c, Some(Command::AttackMove(t)));
        assert!(!ui.pending_amove);
    }

    #[test]
    fn the_briefing_shows_first_and_any_key_closes_it_without_acting() {
        let (mut ui, g) = (Ui::new(), Game::new(1));
        assert!(ui.show_help, "new players see the briefing first");
        // pressing '1' dismisses it but does NOT train by accident
        assert!(matches!(key(&mut ui, &g, '1'), KeyAction::None));
        assert!(!ui.show_help);
        // now keys work normally
        assert!(matches!(
            key(&mut ui, &g, '1'),
            KeyAction::Command(Command::Train(Kind::Cadet))
        ));
    }

    #[test]
    fn question_mark_reopens_the_briefing_and_q_still_quits_through_it() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        assert!(matches!(key(&mut ui, &g, '?'), KeyAction::None));
        assert!(ui.show_help);
        assert!(matches!(key(&mut ui, &g, 'q'), KeyAction::Quit));
        let mut ui = Ui::new();
        let ctrl_c = handle_key(
            &mut ui,
            &g,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            area(),
        );
        assert!(matches!(ctrl_c, KeyAction::Quit));
    }

    #[test]
    fn a_click_closes_the_briefing_without_ordering_anything() {
        let (mut ui, g) = (Ui::new(), Game::new(1));
        let at = (SHIP.0 + 6, SHIP.1 + 4);
        let click = mouse(MouseEventKind::Down(MouseButton::Right), at.0, at.1);
        assert_eq!(handle_mouse(&mut ui, &g, click, area()), None);
        assert!(!ui.show_help);
        assert_eq!(
            handle_mouse(&mut ui, &g, click, area()),
            Some(Command::Move(at))
        );
    }

    #[test]
    fn the_briefing_and_every_screen_draw_on_small_and_large_terminals() {
        use ratatui::{Terminal, backend::TestBackend};
        let g = Game::new(1);
        for (w, h) in [(100, 30), (140, 50), (60, 20), (20, 6), (1, 1)] {
            for show_help in [true, false] {
                let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                let mut ui = Ui::new();
                ui.show_help = show_help;
                terminal.draw(|f| draw(f, &g, &ui)).unwrap();
            }
        }
    }

    #[test]
    fn long_messages_wrap_instead_of_being_cut_off() {
        let msg = "[2:14] SURGE inbound from the south. 5 contacts, 15 seconds. Fortify!";
        let lines = wrap(msg, 30);
        assert!(lines.len() >= 3);
        assert!(lines.iter().all(|l| l.chars().count() <= 30), "{lines:?}");
        // nothing lost, continuation lines indented
        let rejoined = lines
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(rejoined, msg);
        assert!(lines[1].starts_with("   "));
        // short messages stay on one line; empty ones vanish
        assert_eq!(wrap("short one", 30), vec!["short one".to_string()]);
        assert!(wrap("", 30).is_empty());
    }

    #[test]
    fn page_keys_scroll_the_log_and_end_returns_to_the_latest() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        let page = |ui: &mut Ui, code| handle_key(ui, &g, code, KeyModifiers::NONE, area());
        assert_eq!(ui.log_scroll, 0);
        page(&mut ui, KeyCode::PageUp);
        page(&mut ui, KeyCode::PageUp);
        assert_eq!(ui.log_scroll, 2 * LOG_PAGE);
        page(&mut ui, KeyCode::PageDown);
        assert_eq!(ui.log_scroll, LOG_PAGE);
        page(&mut ui, KeyCode::End);
        assert_eq!(ui.log_scroll, 0);
        // can't scroll below the latest message
        page(&mut ui, KeyCode::PageDown);
        assert_eq!(ui.log_scroll, 0);
    }

    fn screen_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn a_small_terminal_is_told_its_size_and_a_big_one_is_not() {
        use ratatui::{Terminal, backend::TestBackend};
        let g = Game::new(1);
        let mut ui = quiet_ui();
        ui.show_help = false;
        let mut small = Terminal::new(TestBackend::new(90, 26)).unwrap();
        small.draw(|f| draw(f, &g, &ui)).unwrap();
        assert!(screen_text(&small).contains("90x26"));
        let mut big = Terminal::new(TestBackend::new(120, 40)).unwrap();
        big.draw(|f| draw(f, &g, &ui)).unwrap();
        assert!(!screen_text(&big).contains("recommended"));
    }

    #[test]
    fn the_comms_log_shows_timestamps_and_history_scrolls() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut g = Game::new(1);
        g.glass = 0;
        for _ in 0..40 {
            g.apply(Command::Train(Kind::Marine)); // each refusal is logged
        }
        let mut ui = quiet_ui();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &g, &ui)).unwrap();
        let latest = screen_text(&terminal);
        assert!(
            latest.contains("[0:00]") || latest.contains("[1:"),
            "timestamps shown"
        );
        assert!(!latest.contains("scrolled back"));
        ui.log_scroll = 100;
        terminal.draw(|f| draw(f, &g, &ui)).unwrap();
        assert!(screen_text(&terminal).contains("scrolled back"));
        // with nothing older to show, scrolling has no effect
        let short = Game::new(1);
        terminal.draw(|f| draw(f, &short, &ui)).unwrap();
        assert!(!screen_text(&terminal).contains("scrolled back"));
    }

    #[test]
    fn building_a_turret_takes_key_four_then_a_click() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        let at = (SHIP.0 + 6, SHIP.1 + 4);
        let left = mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1);
        // a plain left click is just the start of a drag, not a build
        assert_eq!(handle_mouse(&mut ui, &g, left, area()), None);
        ui.drag = None;
        assert!(matches!(key(&mut ui, &g, '4'), KeyAction::None));
        assert!(ui.pending_build);
        assert_eq!(
            handle_mouse(&mut ui, &g, left, area()),
            Some(Command::Build(at))
        );
        assert!(!ui.pending_build, "one click, one turret");
    }

    #[test]
    fn building_can_be_cancelled_with_esc_or_a_right_click() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        key(&mut ui, &g, '4');
        let esc = handle_key(&mut ui, &g, KeyCode::Esc, KeyModifiers::NONE, area());
        assert!(matches!(esc, KeyAction::Command(Command::Deselect)));
        assert!(!ui.pending_build);
        key(&mut ui, &g, '4');
        let right = mouse(
            MouseEventKind::Down(MouseButton::Right),
            SHIP.0 + 6,
            SHIP.1 + 4,
        );
        assert_eq!(handle_mouse(&mut ui, &g, right, area()), None);
        assert!(!ui.pending_build);
        // starting an attack-move also drops build mode, and vice versa
        key(&mut ui, &g, '4');
        let mut g2 = Game::new(1);
        g2.apply(Command::SelectAll);
        key(&mut ui, &g2, 'a');
        assert!(ui.pending_amove && !ui.pending_build);
        key(&mut ui, &g2, '4');
        assert!(ui.pending_build && !ui.pending_amove);
    }

    #[test]
    fn select_by_kind_keys_issue_commands() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        assert!(matches!(
            key(&mut ui, &g, 'e'),
            KeyAction::Command(Command::SelectKind(Kind::Engineer))
        ));
        assert!(matches!(
            key(&mut ui, &g, 'm'),
            KeyAction::Command(Command::SelectKind(Kind::Marine))
        ));
    }

    #[test]
    fn the_sidebar_lists_crew_by_type_and_flags_no_engineers() {
        use ratatui::{Terminal, backend::TestBackend};
        let ui = quiet_ui();
        let mut g = Game::new(1);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &g, &ui)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Cad 2  Eng 1  Mar 1"), "roster missing");
        assert!(!text.contains("Down"));
        // the lone Engineer goes Down: count drops to zero and Down shows
        let e = g
            .units
            .iter()
            .position(|u| u.kind == Kind::Engineer)
            .unwrap();
        g.units[e].down = Some(100);
        terminal.draw(|f| draw(f, &g, &ui)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Eng 0"), "zero Engineers must be visible");
        assert!(text.contains("Down 1"));
    }

    #[test]
    fn the_whole_sidebar_fits_the_recommended_minimum_terminal() {
        use ratatui::{Terminal, backend::TestBackend};
        let (g, ui) = (Game::new(1), quiet_ui());
        let mut terminal = Terminal::new(TestBackend::new(MIN_COLS, MIN_ROWS)).unwrap();
        terminal.draw(|f| draw(f, &g, &ui)).unwrap();
        let text = screen_text(&terminal);
        // first and last sidebar rows, and the new roster and training rows
        for needle in ["Driftglass", "Cad 2  Eng 1  Mar 1", "[4] Turret", "q: quit"] {
            assert!(text.contains(needle), "{needle:?} is cut off at 100x30");
        }
    }

    #[test]
    fn drag_box_becomes_a_normalised_selection() {
        let (mut ui, g) = (quiet_ui(), Game::new(1));
        let (a, b) = ((SHIP.0 + 8, SHIP.1 + 5), (SHIP.0 + 2, SHIP.1 + 1));
        let down = mouse(MouseEventKind::Down(MouseButton::Left), a.0, a.1);
        assert_eq!(handle_mouse(&mut ui, &g, down, area()), None);
        let drag = mouse(MouseEventKind::Drag(MouseButton::Left), b.0, b.1);
        assert_eq!(handle_mouse(&mut ui, &g, drag, area()), None);
        assert!(ui.drag.is_some());
        let up = mouse(MouseEventKind::Up(MouseButton::Left), b.0, b.1);
        // dragging up-and-left must still give min <= max
        assert_eq!(
            handle_mouse(&mut ui, &g, up, area()),
            Some(Command::SelectBox {
                min: (SHIP.0 + 2, SHIP.1 + 1),
                max: (SHIP.0 + 8, SHIP.1 + 5),
            })
        );
        assert!(ui.drag.is_none());
    }
}
