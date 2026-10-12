//! Tests for the live request view state and renderer (S4.6/S4.7).

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use pumice::monitor::{EventKind, ParsedText};
use pumice::pipeline::{AttemptResult, OutcomeKind, RawReason};
use pumice::providers::ProviderError;
use pumice::tui::app::{Action, App, State, Tab};
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

fn arrived(
    number: u64,
    base: Instant,
    offset_ms: u64,
    client: Option<&str>,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Arrived {
            client: client.map(str::to_owned),
        },
    }
}

fn parsed(
    number: u64,
    base: Instant,
    offset_ms: u64,
    model: &str,
    input: &str,
) -> pumice::monitor::Event {
    parsed_full(
        number,
        base,
        offset_ms,
        model,
        ParsedText {
            system: vec![],
            before: String::new(),
            input: input.to_owned(),
            after: String::new(),
        },
    )
}

fn parsed_full(
    number: u64,
    base: Instant,
    offset_ms: u64,
    model: &str,
    text: ParsedText,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Parsed {
            model: Some(model.to_owned()),
            text: Some(text),
        },
    }
}

fn received(
    number: u64,
    base: Instant,
    offset_ms: u64,
    headers: Vec<(&str, &str)>,
    body: &str,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Received {
            headers: headers
                .into_iter()
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect(),
            body: body.to_owned(),
        },
    }
}

fn sent(
    number: u64,
    base: Instant,
    offset_ms: u64,
    status: u16,
    headers: Vec<(&str, &str)>,
    body: &str,
) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Sent {
            status,
            headers: headers
                .into_iter()
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect(),
            body: body.to_owned(),
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

fn queued(number: u64, base: Instant, offset_ms: u64) -> pumice::monitor::Event {
    pumice::monitor::Event {
        number,
        at: base + Duration::from_millis(offset_ms),
        kind: EventKind::Queued,
    }
}

#[test]
fn states_for_each_lifecycle_stage() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0, None));
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

    app.apply(arrived(2, base, 50, None));
    app.apply(dropped(2, base, 60));
    let req = app
        .rows()
        .into_iter()
        .find(|r| r.number == 2)
        .expect("dropped request");
    assert!(matches!(req.state(), State::Dropped));

    app.apply(arrived(3, base, 70, None));
    app.apply(parsed(3, base, 80, "claude", "ola"));
    app.apply(queued(3, base, 90));
    let req = app
        .rows()
        .into_iter()
        .find(|r| r.number == 3)
        .expect("queued request");
    assert!(matches!(req.state(), State::Queued));
}

#[test]
fn wait_cli_total_durations() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.apply(arrived(1, base, 0, None));
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
    app.apply(arrived(2, base, 0, None));
    app.apply(started(2, base, 50));
    app.apply(arrived(3, base, 0, None));
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
    app.apply(arrived(1, base, 0, None));
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

    app.apply(arrived(1, base, 0, None));
    assert_eq!(app.selected().map(|r| r.number), Some(1));

    app.apply(arrived(2, base, 10, None));
    assert_eq!(app.selected().map(|r| r.number), Some(2));

    // Move selection to the older request.
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected().map(|r| r.number), Some(1));

    // A new request arrives; selection stays on #1.
    app.apply(arrived(3, base, 20, None));
    assert_eq!(app.selected().map(|r| r.number), Some(1));
}

#[test]
fn filter_cycles_all_failed_and_models() {
    let base = Instant::now();
    let mut app = App::new(info(false));

    app.apply(arrived(1, base, 0, None));
    app.apply(parsed(1, base, 10, "claude", "ola"));
    app.apply(responded(1, base, 20, 200, "formatted", None));

    app.apply(arrived(2, base, 30, None));
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

    app.apply(arrived(1, base, 0, None));
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
    app.apply(arrived(1, base, 0, None));
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
        app.apply(arrived(n, base, n * 10, None));
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

    app.apply(arrived(1, base, 0, None));
    app.apply(parsed(1, base, 10, "claude", "ola"));
    app.apply(started(1, base, 20));

    app.apply(arrived(2, base, 30, None));
    app.apply(parsed(2, base, 40, "claude", "ola"));
    app.apply(started(2, base, 50));
    app.apply(attempt_ended(
        2,
        base,
        60,
        AttemptResult::Failed(ProviderError::Timeout),
    ));
    app.apply(responded(2, base, 70, 504, "timed out", None));

    let content = screen(&app, base + Duration::from_millis(100));
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
        app.apply(arrived(1, base, 0, None));
        app.apply(parsed(1, base, 10, "claude", input));
        app.apply(responded(1, base, 20, 200, "formatted", Some(reply)));
        app.key(key(KeyCode::Enter));

        let content = screen(&app, base + Duration::from_millis(100));
        assert!(
            content.contains(expected),
            "expected {expected:?} when text_allowed={text_allowed}: {content}"
        );
    }
}

/// The whole screen as text, one line per row.
fn screen(app: &App, now: Instant) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
    terminal
        .draw(|frame| {
            let result = view::draw(frame, app, now);
            // Tests that drive the app directly do not feed bounds back, so
            // scrolling is exercised through set_details_bounds where needed.
            let _ = result;
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_dropped_request_stops_its_clocks() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    // #1 was running its CLI when the client went away.
    app.apply(arrived(1, base, 0, None));
    app.apply(started(1, base, 1000));
    app.apply(dropped(1, base, 2000));
    // #2 was still waiting in line.
    app.apply(arrived(2, base, 0, None));
    app.apply(queued(2, base, 500));
    app.apply(dropped(2, base, 1500));

    let later = base + Duration::from_secs(10);
    let rows = app.rows();
    let (second, first) = (rows[0], rows[1]);
    assert_eq!(first.cli(later), Some(Duration::from_secs(1)));
    assert_eq!(first.total(later), Some(Duration::from_secs(2)));
    assert_eq!(second.wait(later), Some(Duration::from_secs(1)));
    assert_eq!(app.running(), 0);
    assert_eq!(app.queued(), 0);

    let screen = screen(&app, later);
    assert!(
        !screen.contains('…'),
        "no live marker on finished rows:\n{screen}"
    );
    assert!(screen.contains("1.0s"), "{screen}");
}

#[test]
fn the_cap_skips_unfinished_requests_and_applies_when_requests_finish() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    // The oldest request is still running while 699 quick ones finish.
    app.apply(arrived(1, base, 0, None));
    app.apply(started(1, base, 1));
    for n in 2..=700 {
        app.apply(arrived(n, base, n, None));
        app.apply(responded(n, base, n, 200, "formatted", None));
    }
    let rows = app.rows();
    assert_eq!(rows.len(), 500);
    assert!(
        rows.iter().any(|r| r.number == 1),
        "the running request stays"
    );
    assert_eq!(rows[0].number, 700);

    // A burst of unfinished requests beyond the cap is trimmed as they end,
    // with no further arrival.
    let mut app = App::new(info(false));
    for n in 1..=600 {
        app.apply(arrived(n, base, n, None));
    }
    assert_eq!(app.rows().len(), 600, "nothing finished: everything stays");
    for n in 1..=600 {
        app.apply(responded(n, base, 1000 + n, 200, "formatted", None));
    }
    assert_eq!(app.rows().len(), 500);
    assert!(app.rows().iter().all(|r| r.number > 100));
}

#[test]
fn filtering_keeps_a_visible_row_selected() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.apply(arrived(1, base, 0, None));
    app.apply(parsed(1, base, 1, "claude", "ola"));
    app.apply(responded(1, base, 2, 504, "timed out", None));
    app.apply(arrived(2, base, 10, None));
    app.apply(parsed(2, base, 11, "claude", "ola"));
    app.apply(responded(2, base, 12, 200, "formatted", None));
    assert_eq!(app.selected().map(|r| r.number), Some(2));

    // Only the older failure is visible: it becomes the selection.
    app.key(char_key('f'));
    assert_eq!(app.filter_name(), "failed");
    assert_eq!(app.selected().map(|r| r.number), Some(1));

    // A new request hidden by the filter does not take the selection away.
    app.apply(arrived(3, base, 20, None));
    app.apply(parsed(3, base, 21, "claude", "ola"));
    assert_eq!(app.selected().map(|r| r.number), Some(1));
    // Once it fails it is visible, and the view follows the newest row.
    app.apply(responded(3, base, 22, 502, "failed", None));
    assert_eq!(app.selected().map(|r| r.number), Some(3));

    // A search that hides the selection moves it to a visible row.
    app.key(char_key('/'));
    for c in "timed".chars() {
        app.key(char_key(c));
    }
    assert_eq!(app.selected().map(|r| r.number), Some(1));
    app.key(key(KeyCode::Esc));
    assert!(app.selected().is_some());
}

#[test]
fn app_column_shows_client_or_dash() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.apply(arrived(1, base, 0, Some("Handy")));
    app.apply(parsed(1, base, 10, "claude", "ola"));
    app.apply(responded(1, base, 20, 200, "formatted", None));

    app.apply(arrived(2, base, 30, None));
    app.apply(parsed(2, base, 40, "codex", "oi"));
    app.apply(responded(2, base, 50, 200, "formatted", None));

    let content = screen(&app, base + Duration::from_millis(100));
    assert!(content.contains("Handy"), "client shown: {content}");
    assert!(
        content.contains(" - "),
        "missing client shown as dash: {content}"
    );
}

#[test]
fn q_in_details_closes_them_and_does_not_quit() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    app.apply(arrived(1, base, 0, None));
    app.apply(parsed(1, base, 10, "claude", "dictado"));
    app.apply(responded(1, base, 20, 200, "formatted", None));
    app.key(key(KeyCode::Enter));
    assert!(app.details_open());

    let action = app.key(char_key('q'));
    assert!(matches!(action, Action::None));
    assert!(!app.details_open());

    let action = app.key(char_key('q'));
    assert!(matches!(action, Action::Quit));
}

#[test]
fn details_scroll_clamps_at_bottom() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    app.apply(arrived(1, base, 0, None));
    app.apply(parsed(1, base, 10, "claude", "dictado"));
    app.apply(responded(1, base, 20, 200, "formatted", Some("Olá.")));
    app.key(key(KeyCode::Enter));

    // Simulate a details area of 10 lines with content of 100 rendered lines.
    app.set_details_bounds(Some(pumice::tui::app::ScrollBounds {
        max_scroll: 90,
        page: 9,
    }));
    for _ in 0..100 {
        app.key(key(KeyCode::Down));
    }
    assert_eq!(app.details_scroll(), 90);
    app.key(key(KeyCode::Up));
    assert_eq!(app.details_scroll(), 89);
}

#[test]
fn tab_backtab_and_digits_cycle_tabs() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    app.apply(arrived(1, base, 0, None));
    app.apply(parsed(1, base, 10, "claude", "dictado"));
    app.apply(responded(1, base, 20, 200, "formatted", None));
    app.key(key(KeyCode::Enter));

    assert_eq!(app.tab(), Tab::Summary);
    app.key(key(KeyCode::Tab));
    assert_eq!(app.tab(), Tab::Received);
    app.key(key(KeyCode::BackTab));
    assert_eq!(app.tab(), Tab::Summary);
    app.key(char_key('3'));
    assert_eq!(app.tab(), Tab::Parsed);
    app.key(char_key('4'));
    assert_eq!(app.tab(), Tab::Sent);
    app.key(char_key('1'));
    assert_eq!(app.tab(), Tab::Summary);
}

#[test]
fn table_page_down_moves_and_clamps() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    for n in 1..=20 {
        app.apply(arrived(n, base, n * 10, None));
        app.apply(parsed(n, base, n * 10 + 1, "claude", "ola"));
        app.apply(responded(n, base, n * 10 + 2, 200, "formatted", None));
    }
    // 20 rows, selection starts at newest (#20, index 0).
    app.set_table_page(5);
    app.key(key(KeyCode::PageDown));
    assert_eq!(app.selected().map(|r| r.number), Some(15));
    app.key(key(KeyCode::PageDown));
    assert_eq!(app.selected().map(|r| r.number), Some(10));
    app.key(key(KeyCode::PageDown));
    assert_eq!(app.selected().map(|r| r.number), Some(5));
    app.key(key(KeyCode::PageDown));
    // Already at the last row (#1), stays there.
    assert_eq!(app.selected().map(|r| r.number), Some(1));
}

/// Draws `app` on a `width`×`height` screen and feeds the scroll bounds back,
/// as the terminal loop does. Returns the screen as text.
fn render(app: &mut App, now: Instant, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut result = None;
    terminal
        .draw(|frame| result = Some(view::draw(frame, app, now)))
        .expect("draw");
    let result = result.expect("drawn");
    app.set_details_bounds(result.details_bounds);
    app.set_help_bounds(result.help_bounds);
    app.set_table_page(result.table_page);
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One finished request #`number` with `input` and a 200 reply.
fn finished_request(app: &mut App, number: u64, base: Instant, input: &str) {
    let offset = number * 100;
    app.apply(arrived(number, base, offset, Some("Handy")));
    app.apply(parsed(number, base, offset + 10, "claude", input));
    app.apply(responded(
        number,
        base,
        offset + 20,
        200,
        "formatted",
        Some("Ok."),
    ));
}

#[test]
fn enter_opens_full_screen_details() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    finished_request(&mut app, 1, base, "dictado");

    let table = render(&mut app, base, 120, 30);
    assert!(table.contains("arrived   app"), "table header: {table}");

    app.key(key(KeyCode::Enter));
    assert!(app.details_open());
    let details = render(&mut app, base, 120, 30);
    assert!(
        !details.contains("arrived   app"),
        "table hidden: {details}"
    );
    assert!(details.contains("Input:"), "{details}");
    assert!(details.contains("dictado"), "{details}");
    assert!(details.contains("arrived from Handy"), "{details}");
}

#[test]
fn left_and_right_follow_table_order_keep_the_tab_and_start_at_the_top() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    for number in 1..=3 {
        finished_request(&mut app, number, base, &"line\n".repeat(100));
    }
    // Rows are newest first: #3, #2, #1. Select the middle one.
    app.key(key(KeyCode::Down));
    app.key(key(KeyCode::Enter));
    app.key(char_key('3'));
    render(&mut app, base, 120, 30);
    app.key(key(KeyCode::PageDown));
    assert!(app.details_scroll() > 0);

    app.key(key(KeyCode::Right));
    assert_eq!(
        app.selected().map(|r| r.number),
        Some(1),
        "next = row below"
    );
    assert_eq!(app.tab(), Tab::Parsed);
    assert_eq!(app.details_scroll(), 0, "a new request starts at its top");

    app.key(key(KeyCode::Right));
    assert_eq!(
        app.selected().map(|r| r.number),
        Some(1),
        "clamped at the end"
    );

    app.key(key(KeyCode::Left));
    app.key(key(KeyCode::Left));
    assert_eq!(
        app.selected().map(|r| r.number),
        Some(3),
        "previous = row above"
    );
    app.key(key(KeyCode::Left));
    assert_eq!(
        app.selected().map(|r| r.number),
        Some(3),
        "clamped at the start"
    );
}

#[test]
fn open_details_hold_their_request_when_a_new_one_arrives() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    finished_request(&mut app, 1, base, "first");
    app.key(key(KeyCode::Enter));
    assert_eq!(app.selected().map(|r| r.number), Some(1));

    app.apply(arrived(2, base, 500, None));
    assert_eq!(app.selected().map(|r| r.number), Some(1));
    let details = render(&mut app, base, 120, 30);
    assert!(details.contains("#1"), "{details}");
}

#[test]
fn every_tab_shows_its_content_with_text_and_hides_it_without() {
    let base = Instant::now();
    let request_body = r#"{"model":"claude","messages":[{"role":"user","content":"ola"}]}"#;
    let response_body = r#"{"id":"chatcmpl-pumice-1","object":"chat.completion"}"#;
    for text_allowed in [false, true] {
        let mut app = App::new(info(text_allowed));
        app.apply(arrived(1, base, 0, Some("Handy")));
        app.apply(received(
            1,
            base,
            5,
            vec![("x-title", "Handy"), ("authorization", "[hidden]")],
            request_body,
        ));
        app.apply(parsed_full(
            1,
            base,
            10,
            "claude",
            ParsedText {
                system: vec!["SYSTEM-PROMPT".to_owned()],
                before: "BEFORE-PART".to_owned(),
                input: "INPUT-PART".to_owned(),
                after: "AFTER-PART".to_owned(),
            },
        ));
        app.apply(responded(1, base, 20, 200, "formatted", Some("REPLY-TEXT")));
        app.apply(sent(
            1,
            base,
            25,
            200,
            vec![("content-type", "application/json")],
            response_body,
        ));
        app.key(key(KeyCode::Enter));

        let expect: [(char, &[&str], &[&str]); 4] = [
            ('1', &["INPUT-PART", "REPLY-TEXT"], &[]),
            (
                '2',
                &[
                    "x-title: Handy",
                    "authorization: [hidden]",
                    "\"model\": \"claude\",",
                ],
                &[],
            ),
            (
                '3',
                &["SYSTEM-PROMPT", "BEFORE-PART", "INPUT-PART", "AFTER-PART"],
                &["Model: claude"],
            ),
            (
                '4',
                &[
                    "content-type: application/json",
                    "\"object\": \"chat.completion\"",
                ],
                &["HTTP 200"],
            ),
        ];
        for (digit, text_parts, always) in expect {
            app.key(char_key(digit));
            let content = render(&mut app, base, 120, 30);
            for part in always {
                assert!(
                    content.contains(part),
                    "tab {digit}: {part} missing: {content}"
                );
            }
            for part in text_parts {
                assert_eq!(
                    content.contains(part),
                    text_allowed,
                    "tab {digit}, text_allowed={text_allowed}, {part}: {content}"
                );
            }
            if !text_allowed {
                assert!(
                    content.contains("Text hidden: debug_log is off."),
                    "{content}"
                );
            }
        }
    }
}

#[test]
fn the_current_tab_is_bold_and_reversed() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    finished_request(&mut app, 1, base, "x");
    app.key(key(KeyCode::Enter));
    app.key(char_key('3'));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
    terminal
        .draw(|frame| {
            view::draw(frame, &app, base);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let row: Vec<_> = (0..120).map(|x| &buffer[(x, 2)]).collect();
    let text: String = row.iter().map(|cell| cell.symbol()).collect();
    let styled = |label: &str| {
        let start = text.find(label).expect("label on the title row");
        let start = text[..start].chars().count();
        row[start]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED | ratatui::style::Modifier::BOLD)
    };
    assert!(styled("3 Parsed"), "{text}");
    assert!(!styled("1 Summary"), "{text}");
}

#[test]
fn scrolling_reaches_the_end_of_more_than_65536_rows() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    let many = |tag: &str| {
        let mut text = (0..30_000).map(|_| "x\n").collect::<String>();
        text.push_str(tag);
        text
    };
    app.apply(arrived(1, base, 0, None));
    app.apply(parsed_full(
        1,
        base,
        10,
        "claude",
        ParsedText {
            system: vec![],
            before: many("END-OF-BEFORE"),
            input: many("END-OF-INPUT"),
            after: many("END-OF-AFTER"),
        },
    ));
    app.key(key(KeyCode::Enter));
    app.key(char_key('3'));
    render(&mut app, base, 80, 20);
    app.key(key(KeyCode::End));
    let content = render(&mut app, base, 80, 20);
    assert!(content.contains("END-OF-AFTER"), "{content}");
    assert!(
        content.contains("/90"),
        "position shows the total: {content}"
    );
}

#[test]
fn help_lists_keys_scrolls_to_the_end_and_closes_without_quitting() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.key(char_key('?'));
    assert!(app.help_open());

    let first = render(&mut app, base, 80, 12);
    let body: String = first.lines().take(11).collect::<Vec<_>>().join("\n");
    assert!(body.contains("PgUp PgDn"), "help body lists paging: {body}");
    assert!(!first.contains("Ctrl-C"), "small screen: last entry below");

    app.key(key(KeyCode::End));
    let last = render(&mut app, base, 80, 12);
    assert!(
        last.contains("Ctrl-C"),
        "End reaches the last entry: {last}"
    );
    let footer = last.lines().last().expect("footer");
    assert!(footer.trim_end().ends_with("? help"), "{footer}");

    assert!(
        matches!(app.key(char_key('q')), Action::None),
        "q closes help"
    );
    assert!(!app.help_open());

    finished_request(&mut app, 1, base, "x");
    app.key(key(KeyCode::Enter));
    app.key(char_key('?'));
    app.key(key(KeyCode::Esc));
    assert!(!app.help_open());
    assert!(app.details_open(), "Esc closes help, not details");
}

#[test]
fn the_footer_keeps_help_visible_with_a_wide_search() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.key(char_key('/'));
    for _ in 0..80 {
        app.key(char_key('界'));
    }
    app.key(key(KeyCode::Enter));
    let content = render(&mut app, base, 120, 10);
    let footer = content.lines().last().expect("footer");
    assert!(footer.trim_end().ends_with("? help"), "{footer}");

    let content = render(&mut app, base, 40, 10);
    let footer = content.lines().last().expect("footer");
    assert!(footer.trim_end().ends_with("? help"), "{footer}");
}

#[test]
fn long_app_names_end_with_an_ellipsis() {
    let base = Instant::now();
    let mut app = App::new(info(false));
    app.apply(arrived(1, base, 0, Some("OpenWhisprLong")));
    let content = render(&mut app, base, 120, 10);
    assert!(content.contains("OpenWhisp…"), "{content}");
}

#[test]
fn sent_does_not_change_state_or_timings() {
    let base = Instant::now();
    let mut app = App::new(info(true));
    finished_request(&mut app, 1, base, "x");
    let now = base + Duration::from_secs(5);
    let before = {
        let req = app.selected().expect("selected");
        (format!("{:?}", req.total(now)), req.finished())
    };
    app.apply(sent(1, base, 4_000, 200, vec![], "{}"));
    let req = app.selected().expect("selected");
    assert!(matches!(req.state(), State::Done(200)));
    assert_eq!((format!("{:?}", req.total(now)), req.finished()), before);
}
