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
//!
//! - [`sim`]: the deterministic, headless simulation.
//! - [`ui`]: drawing and input decoding (input becomes `Command`s).
//! - this file: terminal setup, the seed, and the real-time loop.

mod sim;
mod ui;

use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
};
use ratatui::{layout::Rect, DefaultTerminal};

use sim::{Game, TICK_MS};
use ui::{KeyAction, Ui};

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    // The seed is the one place wall-clock time is allowed to touch the game.
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let mut g = Game::new(seed);
    let mut ui = Ui::new();
    let tick = Duration::from_millis(TICK_MS);
    let mut last = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, &g, &ui))?;
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        if event::poll(tick.saturating_sub(last.elapsed()))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    match ui::handle_key(&mut ui, &g, k.code, k.modifiers, area) {
                        KeyAction::Quit => return Ok(()),
                        KeyAction::Command(c) => g.apply(c),
                        KeyAction::None => {}
                    }
                }
                Event::Mouse(m) => {
                    if let Some(c) = ui::handle_mouse(&mut ui, &g, m, area) {
                        g.apply(c);
                    }
                }
                _ => {}
            }
        }
        if last.elapsed() >= tick {
            g.update();
            last = Instant::now();
        }
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
