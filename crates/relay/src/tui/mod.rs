//! Live interactive terminal view of completion requests (S4.6).
//!
//! When `pumice` runs in a terminal it shows every request's lifecycle:
//! arrived → queued → CLI started → CLI ended → responded/dropped. Dictated
//! text and replies are shown only when the debug log is enabled; otherwise
//! only metadata is visible. The module is split into a pure state part
//! ([`app`]) and a renderer ([`view`]).

pub mod app;
pub mod view;

use std::io;
use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::monitor::Event as MonitorEvent;

/// Startup information shown in the live view header.
pub struct Info {
    pub version: &'static str,
    pub url: String,
    pub max_parallel: usize,
    pub text_allowed: bool,
}

/// Runs the terminal view until the event channel disconnects or the user
/// quits. Sends on `stop` the first time the user asks to quit; the service
/// then drains and the receiver disconnects, which ends the loop cleanly.
/// The terminal is restored on every return path; a panic restores it
/// through the hook `ratatui::init` installs.
pub fn run(
    events: mpsc::Receiver<MonitorEvent>,
    info: Info,
    stop: tokio::sync::oneshot::Sender<()>,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, events, info, stop);
    ratatui::restore();
    result
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    events: mpsc::Receiver<MonitorEvent>,
    info: Info,
    stop: tokio::sync::oneshot::Sender<()>,
) -> io::Result<()> {
    let mut app = app::App::new(info);
    let mut stop = Some(stop);
    loop {
        // Drain every pending event before drawing.
        loop {
            match events.try_recv() {
                Ok(event) => app.apply(event),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        let now = std::time::Instant::now();
        let mut result = None;
        terminal.draw(|frame| {
            result = Some(view::draw(frame, &app, now));
        })?;
        if let Some(result) = result {
            app.set_details_bounds(result.details_bounds);
            app.set_help_bounds(result.help_bounds);
            app.set_table_page(result.table_page);
        }

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        // Windows reports key releases too; only presses count.
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if let app::Action::Quit = app.key(key) {
            match stop.take() {
                Some(sender) => {
                    let _ = sender.send(());
                    app.set_stopping();
                }
                // A second quit while draining: leave at once.
                None => {
                    ratatui::restore();
                    std::process::exit(130);
                }
            }
        }
    }
}
