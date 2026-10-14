//! Renderer for the live request view (S4.6).
//!
//! All drawing code lives here so [`app`](super::app) stays terminal-free and
//! testable.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};

use crate::pipeline::AttemptResult;

use super::Info;
use super::app::{App, Received, RequestView, ScrollBounds, Sent, Started, State, Tab};

/// Result of a draw pass: geometry the app needs to clamp scrolling and
/// paging.
pub struct DrawResult {
    /// Set when the details view was drawn.
    pub details_bounds: Option<ScrollBounds>,
    /// Set when the help screen was drawn.
    pub help_bounds: Option<ScrollBounds>,
    /// Table rows one PageUp/PageDown moves (1 when the table is hidden).
    pub table_page: usize,
}

/// Draws the full live view for `app` at `now`.
pub fn draw(frame: &mut Frame, app: &App, now: Instant) -> DrawResult {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header_area, app);
    let mut result = DrawResult {
        details_bounds: None,
        help_bounds: None,
        table_page: 1,
    };
    if app.help_open() {
        result.help_bounds = Some(draw_help(frame, body_area, app.help_scroll()));
    } else if app.details_open() {
        if let Some(selected) = app.selected() {
            let bounds = draw_details(
                frame,
                body_area,
                app.info(),
                selected,
                app.details_scroll(),
                app.tab(),
            );
            result.details_bounds = Some(bounds);
        }
    } else {
        result.table_page = draw_table(frame, body_area, app, now);
    }
    draw_footer(frame, footer_area, app);
    result
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let info = app.info();
    let text_on = if info.text_allowed { "on" } else { "off" };
    if app.stopping() {
        let waiting = app.waiting();
        let line = Line::from(vec![
            Span::raw(format!("pumice {}  {}  ", info.version, info.url)),
            Span::styled(
                format!("stopping: waiting for {waiting} requests"),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), top);
    } else {
        let line = format!(
            "pumice {}  {}  slots {}/{}  queue {}  ok {}  failed {}  dropped {}  text {}",
            info.version,
            info.url,
            app.running(),
            info.max_parallel,
            app.queued(),
            app.ok(),
            app.failed(),
            app.dropped(),
            text_on,
        );
        frame.render_widget(Paragraph::new(line), top);
    }

    let stats = app.provider_stats();
    let bottom_text = if stats.is_empty() {
        "no CLI runs yet".to_owned()
    } else {
        stats
            .iter()
            .map(|(provider, count, avg)| {
                format!("{} {:.1}s ×{}", provider, avg.as_secs_f64(), count)
            })
            .collect::<Vec<_>>()
            .join("   ")
    };
    frame.render_widget(Paragraph::new(bottom_text), bottom);
}

fn draw_table(frame: &mut Frame, area: Rect, app: &App, now: Instant) -> usize {
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("arrived"),
        Cell::from("app"),
        Cell::from("model"),
        Cell::from("state"),
        Cell::from("wait"),
        Cell::from("cli"),
        Cell::from("total"),
        Cell::from("detail"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app.rows().iter().map(|req| table_row(req, now)).collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(13),
        Constraint::Length(14),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Min(0),
    ];

    let mut state = TableState::default();
    let rows_vec: Vec<&RequestView> = app.rows();
    if let Some(selected) = app.selected()
        && let Some(index) = rows_vec.iter().position(|r| r.number == selected.number)
    {
        state.select(Some(index));
    }

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_stateful_widget(table, area, &mut state);
    area.height.saturating_sub(1).max(1) as usize
}

fn table_row(req: &RequestView, now: Instant) -> Row<'_> {
    let arrived = req.arrived_wall.strftime("%H:%M:%S").to_string();
    let client = req
        .client
        .as_ref()
        .map(|c| truncate_app(c))
        .unwrap_or_else(|| "-".to_owned());
    let model = req
        .model
        .as_ref()
        .map(|m| truncate_model(m))
        .unwrap_or_else(|| "-".to_owned());
    let (state_text, state_color) = state_render(req.state());
    let wait = fmt_duration_opt(req.wait(now), req.waiting_in_line());
    let cli = fmt_duration_opt(req.cli(now), req.cli_running());
    let total = fmt_duration_opt(req.total(now), !req.finished());
    let detail = req
        .responded
        .as_ref()
        .map(|r| r.detail.clone())
        .unwrap_or_default();

    Row::new(vec![
        Cell::from(req.number.to_string()),
        Cell::from(arrived),
        Cell::from(client),
        Cell::from(model),
        Cell::from(Span::styled(state_text, Style::default().fg(state_color))),
        Cell::from(wait),
        Cell::from(cli),
        Cell::from(total),
        Cell::from(truncate_tail(&detail, 40)),
    ])
}

fn draw_details(
    frame: &mut Frame,
    area: Rect,
    info: &Info,
    req: &RequestView,
    scroll: usize,
    tab: Tab,
) -> ScrollBounds {
    let lines = match tab {
        Tab::Summary => summary_lines(req, info),
        Tab::Received => received_lines(req, info),
        Tab::Parsed => parsed_lines(req, info),
        Tab::Sent => sent_lines(req, info),
    };
    let mut title = vec![Span::raw(format!(" #{}  ", req.number))];
    title.extend(tab_bar(tab));
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title));
    draw_scrolled(frame, area, block, lines, scroll)
}

/// Draws `lines` wrapped inside `block`, starting `scroll` rendered rows
/// down, with a `line X/Y` position on the bottom border. Returns the
/// bounds for the next scroll key. Offsets are `usize` throughout: ratatui's
/// own scroll is a `u16`, so whole lines above the offset are skipped here
/// and only the remainder inside one line goes to ratatui.
fn draw_scrolled(
    frame: &mut Frame,
    area: Rect,
    block: Block,
    lines: Vec<Line>,
    scroll: usize,
) -> ScrollBounds {
    let inner = block.inner(area);
    let heights: Vec<usize> = lines
        .iter()
        .map(|line| {
            Paragraph::new(line.clone())
                .wrap(Wrap { trim: false })
                .line_count(inner.width)
        })
        .collect();
    let total: usize = heights.iter().sum();
    let visible = inner.height as usize;
    let max_scroll = total.saturating_sub(visible);
    let page = visible.saturating_sub(1).max(1);
    let scroll = scroll.min(max_scroll);

    let mut skipped = 0;
    let mut first = 0;
    while first < heights.len() && skipped + heights[first] <= scroll {
        skipped += heights[first];
        first += 1;
    }
    let within = u16::try_from(scroll - skipped).unwrap_or(u16::MAX);

    let position = format!(" line {}/{} ", scroll + 1, total.max(1));
    frame.render_widget(
        block.title_bottom(Line::from(position).alignment(Alignment::Right)),
        area,
    );
    let shown: Vec<Line> = lines.into_iter().skip(first).collect();
    frame.render_widget(
        Paragraph::new(Text::from(shown))
            .wrap(Wrap { trim: false })
            .scroll((within, 0)),
        inner,
    );
    ScrollBounds { max_scroll, page }
}

/// The tab labels for the details title; the current one bold and reversed.
fn tab_bar(current: Tab) -> Vec<Span<'static>> {
    let tabs = [Tab::Summary, Tab::Received, Tab::Parsed, Tab::Sent];
    let mut spans = Vec::new();
    for (index, tab) in tabs.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let label = format!(" {} {} ", index + 1, tab.name());
        if tab == current {
            spans.push(Span::styled(
                label,
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
        } else {
            spans.push(Span::raw(label));
        }
    }
    spans.push(Span::raw(" "));
    spans
}

/// The Summary tab: the timeline, then the input and the reply.
fn summary_lines<'a>(req: &'a RequestView, info: &Info) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    let mut arrived_label = "arrived".to_owned();
    if let Some(client) = &req.client {
        arrived_label.push_str(&format!(" from {client}"));
    }
    lines.push(timeline_line(req, req.arrived, arrived_label));

    if let Some(parsed) = req.parsed {
        let label = match &req.model {
            Some(model) => format!("parsed (model {})", model.trim()),
            None => "parsed (no model)".to_owned(),
        };
        lines.push(timeline_line(req, parsed, label));
    }

    if let Some(queued) = req.queued {
        lines.push(timeline_line(req, queued, "queued".to_owned()));
    }

    if let Some(Started {
        at,
        provider,
        model,
    }) = &req.started
    {
        lines.push(timeline_line(
            req,
            *at,
            format!("CLI started {provider} ({model})"),
        ));
    }

    if let Some(attempt) = &req.attempt {
        let label = format!("CLI ended: {}", attempt_result_text(attempt.result));
        lines.push(timeline_line(req, attempt.at, label));
        if let Some(summary) = attempt.diagnostic.as_ref().and_then(|d| d.summary()) {
            lines.push(Line::from(format!("  diagnostic: {summary}")));
        }
    }

    if let Some(resp) = &req.responded {
        let label = format!("responded {} {}", resp.status, resp.detail);
        lines.push(timeline_line(req, resp.at, label));
    }

    if let Some(dropped) = req.dropped {
        lines.push(timeline_line(req, dropped, "dropped".to_owned()));
    }

    lines.push(Line::default());

    if info.text_allowed {
        if let Some(text) = &req.text {
            lines.push(bold_line("Input:"));
            for line in text.input.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Cyan),
                )));
            }
            lines.push(Line::default());
        }
        if let Some(reply) = req.responded.as_ref().and_then(|r| r.reply.as_ref()) {
            lines.push(bold_line("Reply:"));
            for line in reply.lines() {
                lines.push(Line::from(line.to_owned()));
            }
        }
    } else {
        lines.push(Line::from("Text hidden: debug_log is off."));
    }

    lines
}

/// The Received tab: headers, then the body as it arrived.
fn received_lines<'a>(req: &'a RequestView, info: &Info) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    if !info.text_allowed {
        lines.push(Line::from("Text hidden: debug_log is off."));
        return lines;
    }
    match &req.received {
        Some(Received { headers, body }) => {
            lines.push(bold_line("Headers:"));
            for (name, value) in headers {
                lines.push(Line::from(format!("{name}: {value}")));
            }
            lines.push(Line::default());
            lines.push(bold_line("Body:"));
            push_body_lines(&mut lines, body);
        }
        None => match (&req.responded, req.dropped) {
            (Some(responded), _) => {
                lines.push(Line::from(format!("Body not read: {}", responded.detail)));
            }
            (None, Some(_)) => lines.push(Line::from("Body not read: the client went away.")),
            (None, None) => lines.push(Line::from("Not received yet.")),
        },
    }
    lines
}

/// The Parsed tab: the model, then the system prompt and the user message
/// split around the input.
fn parsed_lines<'a>(req: &'a RequestView, info: &Info) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let model = req.model.as_deref().unwrap_or("-");
    lines.push(Line::from(format!("Model: {model}")));

    if !info.text_allowed {
        if req.parsed.is_none() {
            push_not_parsed(&mut lines, req);
        } else {
            lines.push(Line::from("Text hidden: debug_log is off."));
        }
        return lines;
    }

    if let Some(text) = &req.text {
        let system = &text.system;
        if system.is_empty() {
            lines.push(bold_line("System prompt:"));
            lines.push(Line::from("(none)"));
        } else {
            let label = if system.len() == 1 {
                "System prompt:".to_owned()
            } else {
                format!("System prompt 1 of {}:", system.len())
            };
            lines.push(bold_line(&label));
            for (i, prompt) in system.iter().enumerate() {
                if i > 0 {
                    lines.push(bold_line(&format!(
                        "System prompt {} of {}:",
                        i + 1,
                        system.len()
                    )));
                }
                for line in prompt.lines() {
                    lines.push(Line::from(line.to_owned()));
                }
            }
        }
        lines.push(Line::default());

        lines.push(bold_line("Before the input:"));
        if text.before.is_empty() {
            lines.push(Line::from("(nothing)"));
        } else {
            for line in text.before.lines() {
                lines.push(Line::from(line.to_owned()));
            }
        }
        lines.push(Line::default());

        lines.push(bold_line("Input:"));
        if text.input.is_empty() {
            lines.push(Line::from("(nothing)"));
        } else {
            for line in text.input.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Cyan),
                )));
            }
        }
        lines.push(Line::default());

        lines.push(bold_line("After the input:"));
        if text.after.is_empty() {
            lines.push(Line::from("(nothing)"));
        } else {
            for line in text.after.lines() {
                lines.push(Line::from(line.to_owned()));
            }
        }
    } else {
        push_not_parsed(&mut lines, req);
    }

    lines
}

/// Why a request has no parsed text: its rejection, its client leaving, or
/// that parsing has not happened yet.
fn push_not_parsed(lines: &mut Vec<Line>, req: &RequestView) {
    let line = match (&req.responded, req.dropped) {
        (Some(responded), _) => format!("Not parsed: {}", responded.detail),
        (None, Some(_)) => "Not parsed: the client went away.".to_owned(),
        (None, None) => "Not parsed yet.".to_owned(),
    };
    lines.push(Line::from(line));
}

/// The Sent tab: the status, headers and body of the response.
fn sent_lines<'a>(req: &'a RequestView, info: &Info) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    if let Some(Sent {
        status,
        headers,
        body,
    }) = &req.sent
    {
        if !info.text_allowed {
            lines.push(Line::from(format!("HTTP {status}")));
            lines.push(Line::from("Text hidden: debug_log is off."));
            return lines;
        }
        lines.push(Line::from(format!("HTTP {status}")));
        lines.push(bold_line("Headers:"));
        for (name, value) in headers {
            lines.push(Line::from(format!("{name}: {value}")));
        }
        lines.push(Line::default());
        lines.push(bold_line("Body:"));
        let is_json = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.starts_with("application/json"))
            .unwrap_or(false);
        if is_json {
            push_body_lines(&mut lines, body);
        } else {
            for line in body.lines() {
                lines.push(Line::from(line.to_owned()));
            }
        }
    } else if req.dropped.is_some() {
        lines.push(Line::from("Never sent: the client went away."));
    } else {
        let status = req.responded.as_ref().map(|r| r.status).unwrap_or(0);
        if status == 0 {
            lines.push(Line::from("Not sent yet."));
        } else {
            lines.push(Line::from(format!("HTTP {status}")));
            if !info.text_allowed {
                lines.push(Line::from("Text hidden: debug_log is off."));
            } else {
                lines.push(Line::from("Not sent yet."));
            }
        }
    }
    lines
}

/// Pushes a body: valid JSON re-indented (when that stays small), anything
/// else as it is.
fn push_body_lines(lines: &mut Vec<Line>, body: &str) {
    let formatted = serde_json::from_str::<serde::de::IgnoredAny>(body)
        .ok()
        .and_then(|_| reindent_json(body));
    for line in formatted.as_deref().unwrap_or(body).lines() {
        lines.push(Line::from(line.to_owned()));
    }
}

/// A bold section label.
fn bold_line(text: &str) -> Line<'static> {
    Line::from(text.to_owned()).patch_style(Style::default().add_modifier(Modifier::BOLD))
}

/// Draws the scrolling list of every key.
fn draw_help(frame: &mut Frame, area: Rect, scroll: usize) -> ScrollBounds {
    let lines: Vec<Line> = vec![
        bold_line("Request list"),
        help_line("↑ ↓  k j", "select the previous / next request"),
        help_line("PgUp PgDn", "move one page"),
        help_line("Home End  g G", "first / last request"),
        help_line("Enter", "open the request's details"),
        help_line("f", "filter: all, failed, then each model"),
        help_line(
            "/",
            "search: number, model, app, provider, detail, input, reply",
        ),
        help_line("Esc", "clear the filter and the search"),
        help_line("q", "quit (waits for running requests; q again quits now)"),
        Line::default(),
        bold_line("Search"),
        help_line("any key", "type into the search"),
        help_line("Backspace", "delete the last character"),
        help_line("Enter", "keep the search and go back to the list"),
        help_line("Esc", "clear the search"),
        Line::default(),
        bold_line("Details"),
        help_line("↑ ↓  k j", "scroll one line"),
        help_line("PgUp PgDn  Space", "scroll one page"),
        help_line("Home End  g G", "top / bottom"),
        help_line("← →  h l", "previous / next request"),
        help_line("Tab  Shift-Tab", "next / previous tab"),
        help_line("1 2 3 4", "Summary, Received, Parsed, Sent tab"),
        help_line("Esc  Enter  q", "back to the list"),
        Line::default(),
        bold_line("Help"),
        help_line("↑ ↓  k j", "scroll one line"),
        help_line("PgUp PgDn  Space", "scroll one page"),
        help_line("Home End  g G", "top / bottom"),
        help_line("?  Esc  q", "close"),
        Line::default(),
        bold_line("Everywhere"),
        help_line("?", "this help (not while typing a search)"),
        help_line("Ctrl-C", "quit"),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Keys ");
    draw_scrolled(frame, area, block, lines, scroll)
}

/// One help row: the keys, padded, then what they do.
fn help_line(keys: &str, action: &str) -> Line<'static> {
    Line::from(format!("  {keys:<18} {action}"))
}

/// Draws the key hints of the current screen. `? help` has its own area on
/// the right, so a long search or narrow terminal never hides it.
fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = if app.help_open() {
        "↑↓ scroll  pgup/pgdn page  esc close  ^c quit".to_owned()
    } else if app.details_open() {
        "↑↓ scroll  pgup/pgdn page  ←→ request  tab/1-4 tabs  esc back".to_owned()
    } else {
        let search = if app.search_mode() || !app.search().is_empty() {
            format!(": {}", app.search())
        } else {
            String::new()
        };
        format!(
            "↑↓ select  pgup/pgdn page  enter details  f filter: {}  / search{}  esc clear  q quit",
            app.filter_name(),
            search,
        )
    };
    let help = "  ? help";
    let [hints_area, help_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(Line::from(help).width() as u16),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(hints), hints_area);
    frame.render_widget(Paragraph::new(help), help_area);
}

fn state_render(state: State) -> (String, Color) {
    match state {
        State::Reading => ("reading".to_owned(), Color::Gray),
        State::Queued => ("queued".to_owned(), Color::Yellow),
        State::Running(provider) => (format!("running {provider}"), Color::Cyan),
        State::Sending => ("sending".to_owned(), Color::Gray),
        State::Done(status) => (format!("✓ {status}"), Color::Green),
        State::Failed(status) => (format!("✗ {status}"), Color::Red),
        State::Dropped => ("dropped".to_owned(), Color::Red),
    }
}

fn attempt_result_text(result: AttemptResult) -> String {
    match result {
        AttemptResult::Formatted => "formatted".to_owned(),
        AttemptResult::Failed(error) => error.to_string(),
        AttemptResult::CleanupRejected(error) => error.to_string(),
    }
}

/// One timeline step: local wall time, offset since arrival, label.
fn timeline_line(req: &RequestView, at: Instant, label: String) -> Line<'static> {
    let offset = at.duration_since(req.arrived);
    let wall = jiff::SignedDuration::try_from(offset)
        .ok()
        .and_then(|offset| req.arrived_wall.checked_add(offset).ok())
        .unwrap_or_else(|| req.arrived_wall.clone());
    Line::from(format!(
        "{}  +{:.2}s  {}",
        wall.strftime("%H:%M:%S"),
        offset.as_secs_f64(),
        label
    ))
}

fn fmt_duration_opt(d: Option<Duration>, live: bool) -> String {
    match d {
        Some(d) => {
            let mut s = format!("{:.1}s", d.as_secs_f64());
            if live {
                s.push('…');
            }
            s
        }
        None => String::new(),
    }
}

/// The app name in its 10-column cell: names longer than that keep nine
/// characters and an ellipsis.
fn truncate_app(app: &str) -> String {
    const MAX: usize = 10;
    if app.chars().count() <= MAX {
        app.to_owned()
    } else {
        let short: String = app.chars().take(MAX - 1).collect();
        format!("{short}…")
    }
}

fn truncate_model(model: &str) -> String {
    const MAX: usize = 12;
    if model.chars().count() <= MAX {
        model.to_owned()
    } else {
        let short: String = model.chars().take(MAX).collect();
        format!("{short}…")
    }
}

fn truncate_tail(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let short: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{short}…")
    }
}

/// Re-indents valid JSON with 2 spaces per level, keeping key order and
/// string contents byte for byte (it never goes through `serde_json::Value`,
/// which would reorder keys). `None` when the result would grow past four
/// times the input plus 4 KiB: deep nesting multiplies indentation, so a
/// small crafted body could otherwise take gigabytes.
fn reindent_json(input: &str) -> Option<String> {
    let budget = input.len().saturating_mul(4).saturating_add(4096);
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut indent: usize = 0;
    // True when the next token is the first non-whitespace on its line and
    // needs the current indent written before it.
    let mut needs_indent = true;

    while i < input.len() {
        if out.len() > budget {
            return None;
        }
        let c = input[i..].chars().next().expect("i is on a char boundary");
        if c == '"' {
            if needs_indent {
                write_indent(&mut out, indent);
            }
            let start = i;
            i += 1;
            let mut escape = false;
            while i < bytes.len() {
                let b = bytes[i];
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    break;
                }
                i += 1;
            }
            let end = if i < bytes.len() { i + 1 } else { i };
            out.push_str(&input[start..end]);
            needs_indent = false;
            i = end;
            continue;
        }
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if c == '{' || c == '[' {
            let close = if c == '{' { '}' } else { ']' };
            let mut j = i + c.len_utf8();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] as char == close {
                if needs_indent {
                    write_indent(&mut out, indent);
                }
                out.push(c);
                out.push(close);
                needs_indent = false;
                i = j + 1;
                continue;
            }
            if needs_indent {
                write_indent(&mut out, indent);
            }
            out.push(c);
            indent += 1;
            out.push('\n');
            write_indent(&mut out, indent);
            needs_indent = false;
            i += 1;
            continue;
        }
        if c == '}' || c == ']' {
            indent = indent.saturating_sub(1);
            out.push('\n');
            write_indent(&mut out, indent);
            out.push(c);
            needs_indent = false;
            i += 1;
            continue;
        }
        if c == ',' {
            out.push(',');
            out.push('\n');
            write_indent(&mut out, indent);
            needs_indent = false;
            i += 1;
            continue;
        }
        if c == ':' {
            out.push(':');
            out.push(' ');
            needs_indent = false;
            i += 1;
            continue;
        }
        if needs_indent {
            write_indent(&mut out, indent);
        }
        out.push(c);
        needs_indent = false;
        i += c.len_utf8();
    }
    Some(out)
}

/// Writes `indent` levels of two spaces.
fn write_indent(out: &mut String, indent: usize) {
    for _ in 0..indent * 2 {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindent_nested_object() {
        let input = r#"{"b":1,"a":{"d":2,"c":[]}}"#;
        let expected = r#"{
  "b": 1,
  "a": {
    "d": 2,
    "c": []
  }
}"#;
        assert_eq!(reindent_json(input).as_deref(), Some(expected));
    }

    #[test]
    fn reindent_keeps_key_order() {
        let input = r#"{"z":1,"a":2,"m":3}"#;
        let expected = r#"{
  "z": 1,
  "a": 2,
  "m": 3
}"#;
        assert_eq!(reindent_json(input).as_deref(), Some(expected));
    }

    #[test]
    fn reindent_strings_with_structure() {
        let input = r#"{"key":"value with {, } and \"quotes\" and \\ backslash and \n newline"}"#;
        let expected = r#"{
  "key": "value with {, } and \"quotes\" and \\ backslash and \n newline"
}"#;
        assert_eq!(reindent_json(input).as_deref(), Some(expected));
    }

    #[test]
    fn reindent_empty_pairs() {
        assert_eq!(reindent_json("{}").as_deref(), Some("{}"));
        assert_eq!(reindent_json("[]").as_deref(), Some("[]"));
        assert_eq!(
            reindent_json(r#"{"a":{},"b":[]}"#).as_deref(),
            Some(
                r#"{
  "a": {},
  "b": []
}"#
            )
        );
    }

    #[test]
    fn reindent_multibyte_char_boundary() {
        let input = r#"{"text":"Olá mundo"}"#;
        let expected = r#"{
  "text": "Olá mundo"
}"#;
        assert_eq!(reindent_json(input).as_deref(), Some(expected));
    }

    #[test]
    fn reindent_gives_up_on_deep_nesting() {
        // 4,001 bytes nested 2,000 deep would indent to about 8 MB.
        let deep = "[".repeat(2000) + "0" + &"]".repeat(2000);
        assert_eq!(reindent_json(&deep), None);
        // Ordinary nesting stays well inside the budget.
        let shallow = "[".repeat(20) + "0" + &"]".repeat(20);
        assert!(reindent_json(&shallow).is_some());
    }
}
