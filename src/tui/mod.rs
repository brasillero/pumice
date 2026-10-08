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
pub fn run(
    events: mpsc::Receiver<MonitorEvent>,
    info: Info,
    stop: tokio::sync::oneshot::Sender<()>,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = app::App::new(info);
    let mut stop = Some(stop);
    let mut quitting = false;

    let result = 'main: loop {
        // Drain every pending event before drawing.
        loop {
            match events.try_recv() {
                Ok(event) => app.apply(event),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'main Ok(()),
            }
        }

        let now = std::time::Instant::now();
        if let Err(error) = terminal.draw(|frame| view::draw(frame, &app, now)) {
            break 'main Err(error);
        }

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.key(key) {
                app::Action::None => {}
                app::Action::Quit => {
                    if let Some(sender) = stop.take() {
                        let _ = sender.send(());
                        app.set_stopping();
                        quitting = true;
                    } else if quitting {
                        ratatui::restore();
                        std::process::exit(130);
                    }
                }
            }
        }
    };

    ratatui::restore();
    result
}
