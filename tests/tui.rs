//! Tests for the live request view state and renderer (S4.6).

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use pumice::monitor::EventKind;
use pumice::pipeline::{AttemptResult, OutcomeKind, RawReason};
use pumice::providers::ProviderError;
use pumice::tui::app::{Action, App, State};
use pumice::tui::view;

fn info(text_allowed: bool) -> pumice::tui::Info {
    pumice::tui::Info {
        version: "0.1.1",
        url: "http://127.0.0.1:7567/v1".to_owned(),
        max_parallel: 4,
        text_allowed,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn arrived(number: u64, base: Instant, offset_ms: u64) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Arrived,
    }
}

fn parsed(
    number: u64,
    base: Instant,
    offset_ms: u64,
    model: &str,
    input: &str,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Parsed {
            model: Some(model.to_owned()),
            input: Some(input.to_owned()),
        },
    }
}

fn started(number: u64, base: Instant, offset_ms: u64) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Started {
            provider: "claude",
            model: "haiku".to_owned(),
        },
    }
}

fn attempt_ended(
    number: u64,
    base: Instant,
    offset_ms: u64,
    result: AttemptResult,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::AttemptEnded {
            result,
            diagnostic: None,
        },
    }
}

fn responded(
    number: u64,
    base: Instant,
    offset_ms: u64,
    status: u16,
    detail: &str,
    reply: Option<&str>,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Responded {
            status,
            outcome: Some(if status < 400 {
                OutcomeKind::Formatted
            } else {
                OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::Timeout))
            }),
            detail: detail.to_owned(),
            reply: reply.map(str::to_owned),
        },
    }
}

fn dropped(number: u64, base: Instant, offset_ms: u64) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Dropped,
    }
}

#[test]
fn states_for_each_lifecycle_stage() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0));
    let req = app.selected().expect("selected");
    assert!(matches!(req.state(), State::Reading));

    app.apply(parsed(1, base, 10, "claude", "ola"));
    let req = app.selected().expect("selected");
    assert!(matches!(req.state(), State::Reading));

    app.apply(started(1, base, 20));
    let req = app.selected().expect("selected");
    assert!(matches!(req.state(), State::Running("claude")));

    app.apply(attempt_ended(1, base, 30, AttemptResult::Formatted));
    let req = app.selected().expect("selected");
    assert!(matches!(req.state(), State::Sending));

    app.apply(responded(1, base, 40, 200, "formatted", Some("Olá.")));
    let req = app.selected().expect("selected");
    assert!(matches!(req.state(), State::Done(200)));

    app.apply(arrived(2, base, 50));
    app.apply(dropped(2, base, 60));
    let req = app
        .rows()
        .into_iter()
        .find(|r| r.number == 2)
        .expect("dropped request");
    assert!(matches!(req.state(), State::Dropped));

    app.apply(arrived(3, base, 70));
    app.apply(parsed(3, base, 80, "claude", "ola"));
    app.apply(queued(3, base, 90));
    let req = app
        .rows()
        .into_iter()
        .find(|r| r.number == 3)
        .expect("queued request");
    assert!(matches!(req.state(), State::Queued));
}

fn queued(number: u64, base: Instant, offset_ms: u64) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Queued,
    }
}

#[test]
fn wait_cli_total_durations() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.apply(arrived(1, base, 0));
    app.apply(queued(1, base, 100));
    app.apply(started(1, base, 300));
    app.apply(attempt_ended(1, base, 800, AttemptResult::Formatted));
    app.apply(responded(1, base, 900, 200, "formatted", None));

    let req = app.selected().expect("selected");
    let now = base + Duration::from_millis(1000);
    // Wait counts from entering the line, not from arrival.
    assert_eq!(req.wait(now), Some(Duration::from_millis(200)));
    assert_eq!(req.cli(now), Some(Duration::from_millis(500)));
    assert_eq!(req.total(now), Some(Duration::from_millis(900)));

    // A request that never queued has no wait; a queued one counts live.
    app.apply(arrived(2, base, 0));
    app.apply(started(2, base, 50));
    app.apply(arrived(3, base, 0));
    app.apply(queued(3, base, 400));
    let rows = app.rows();
    let (third, second) = (rows[0], rows[1]);
    assert_eq!(second.wait(now), None);
    assert_eq!(third.wait(now), Some(Duration::from_millis(600)));
}

#[test]
fn header_counters_and_provider_averages() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    // One successful claude run.
    app.apply(arrived(1, base, 0));
    app.apply(started(1, base, 10));
    app.apply(attempt_ended(1, base, 110, AttemptResult::Formatted));
    app.apply(responded(1, base, 120, 200, "formatted", None));

    assert_eq!(app.running(), 0);
    assert_eq!(app.queued(), 0);
    assert_eq!(app.ok(), 1);
    assert_eq!(app.failed(), 0);
    assert_eq!(app.dropped(), 0);

    let stats = app.provider_stats();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].0, "claude");
    assert_eq!(stats[0].1, 1);
    assert_eq!(stats[0].2, Duration::from_millis(100));
}

#[test]
fn selection_auto_follows_newest_then_stays() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0));
    assert_eq!(app.selected().map(|r| r.number), Some(1));

    app.apply(arrived(2, base, 10));
    assert_eq!(app.selected().map(|r| r.number), Some(2));

    // Move selection to the older request.
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected().map(|r| r.number), Some(1));

    // A new request arrives; selection stays on #1.
    app.apply(arrived(3, base, 20));
    assert_eq!(app.selected().map(|r| r.number), Some(1));
}

#[test]
fn filter_cycles_all_failed_and_models() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0));
    app.apply(parsed(1, base, 10, "claude", "ola"));
    app.apply(responded(1, base, 20, 200, "formatted", None));

    app.apply(arrived(2, base, 30));
    app.apply(parsed(2, base, 40, "codex", "oi"));
    app.apply(responded(2, base, 50, 502, "failed", None));

    assert_eq!(app.rows().len(), 2);

    app.key(char_key('f'));
    assert_eq!(app.filter_name(), "failed");
    assert_eq!(app.rows().len(), 1);
    assert_eq!(app.rows()[0].number, 2);

    // Models come in the order they were first requested: oldest first.
    app.key(char_key('f'));
    assert_eq!(app.filter_name(), "model:claude");
    assert_eq!(app.rows().len(), 1);
    assert_eq!(app.rows()[0].number, 1);

    app.key(char_key('f'));
    assert_eq!(app.filter_name(), "model:codex");

    app.key(char_key('f'));
    assert_eq!(app.filter_name(), "all");
    assert_eq!(app.rows().len(), 2);
}

#[test]
fn search_typing_q_does_not_quit_and_matches_text_only_when_allowed() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0));
    app.apply(parsed(1, base, 10, "claude", "dictado-secreto"));

    app.key(char_key('/'));
    assert!(app.search_mode());

    // Typing 'q' should add to the query, not quit.
    let action = app.key(char_key('q'));
    assert!(matches!(action, Action::None));
    assert_eq!(app.search(), "q");

    app.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.search(), "");

    // With text hidden, searching the input should find nothing.
    app.key(char_key('d'));
    app.key(char_key('i'));
    app.key(char_key('c'));
    app.key(char_key('t'));
    assert_eq!(app.rows().len(), 0);

    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.search_mode());

    // With text allowed, the same search matches.
    let mut app = App::new(info(true));
    app.apply(arrived(1, base, 0));
    app.apply(parsed(1, base, 10, "claude", "dictado-secreto"));
    app.key(char_key('/'));
    for c in ['d', 'i', 'c', 't'] {
        app.key(char_key(c));
    }
    assert_eq!(app.rows().len(), 1);
}

#[test]
fn q_returns_quit_action() {
    let mut app = App::new(info(false));
    assert!(matches!(app.key(char_key('q')), Action::Quit));
    assert!(matches!(app.key(ctrl_c()), Action::Quit));
}

#[test]
fn five_hundred_request_cap_drops_oldest_finished() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    for n in 1..=550 {
        app.apply(arrived(n, base, n * 10));
        if n <= 50 {
            app.apply(responded(n, base, n * 10 + 1, 200, "formatted", None));
        }
    }
    assert_eq!(app.rows().len(), 500);
    // The oldest finished ones (1..=50) should have been dropped.
    assert!(app.rows().iter().all(|r| r.number > 50));
}

#[test]
fn render_shows_header_running_row_and_failed_status() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0));
    app.apply(parsed(1, base, 10, "claude", "ola"));
    app.apply(started(1, base, 20));

    app.apply(arrived(2, base, 30));
    app.apply(parsed(2, base, 40, "claude", "ola"));
    app.apply(started(2, base, 50));
    app.apply(attempt_ended(
        2,
        base,
        60,
        AttemptResult::Failed(ProviderError::Timeout),
    ));
    app.apply(responded(2, base, 70, 504, "timed out", None));

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| view::draw(frame, &app, base + Duration::from_millis(100)))
        .expect("draw");

    let buf = terminal.backend().buffer();
    let content: String = buf.content.iter().map(|c| c.symbol()).collect();
    assert!(
        content.contains("pumice 0.1.1"),
        "header shows version: {content}"
    );
    assert!(
        content.contains("running claude"),
        "running row present: {content}"
    );
    assert!(
        content.contains("✗ 504"),
        "failed status present: {content}"
    );
}

#[test]
fn details_pane_hides_text_when_debug_log_is_off_and_shows_it_when_on() {
    let base = Instant::now();
    let input = "dictado-secreto";
    let reply = "Resposta.";

    for (text_allowed, expected) in [(false, "Text hidden: debug_log is off."), (true, input)] {
        let mut app = App::new(info(text_allowed));
        app.apply(arrived(1, base, 0));
        app.apply(parsed(1, base, 10, "claude", input));
        app.apply(responded(1, base, 20, 200, "formatted", Some(reply)));
        app.key(key(KeyCode::Enter));

        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| view::draw(frame, &app, base + Duration::from_millis(100)))
            .expect("draw");

        let buf = terminal.backend().buffer();
        let content: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains(expected),
            "expected {expected:?} when text_allowed={text_allowed}: {content}"
        );
    }
}
