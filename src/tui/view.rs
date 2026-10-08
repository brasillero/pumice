//! Renderer for the live request view (S4.6).
//!
//! All drawing code lives here so [`app`](super::app) stays terminal-free and
//! testable.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};

use crate::pipeline::AttemptResult;

use super::Info;
use super::app::{App, RequestView, Started, State};

/// Draws the full live view for `app` at `now`.
pub fn draw(frame: &mut Frame, app: &App, now: Instant) {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header_area, app);
    if app.details_open() {
        let [table_area, details_area] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(body_area);
        draw_table(frame, table_area, app, now);
        if let Some(selected) = app.selected() {
            draw_details(
                frame,
                details_area,
                app.info(),
                selected,
                app.details_scroll(),
            );
        }
    } else {
        draw_table(frame, body_area, app, now);
    }
    draw_footer(frame, footer_area, app);
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

fn draw_table(frame: &mut Frame, area: Rect, app: &App, now: Instant) {
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("arrived"),
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
}

fn table_row(req: &RequestView, now: Instant) -> Row<'_> {
    let arrived = req.arrived_wall.strftime("%H:%M:%S").to_string();
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
        Cell::from(model),
        Cell::from(Span::styled(state_text, Style::default().fg(state_color))),
        Cell::from(wait),
        Cell::from(cli),
        Cell::from(total),
        Cell::from(truncate_tail(&detail, 40)),
    ])
}

fn draw_details(frame: &mut Frame, area: Rect, info: &Info, req: &RequestView, scroll: usize) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" #{} details ", req.number));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(timeline_line(req, req.arrived, "arrived".to_owned()));

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
        if let Some(input) = &req.input {
            lines.push(
                Line::from("Input:").patch_style(Style::default().add_modifier(Modifier::BOLD)),
            );
            for line in input.lines() {
                lines.push(Line::from(line.to_owned()));
            }
            lines.push(Line::default());
        }
        if let Some(reply) = req.responded.as_ref().and_then(|r| r.reply.as_ref()) {
            lines.push(
                Line::from("Reply:").patch_style(Style::default().add_modifier(Modifier::BOLD)),
            );
            for line in reply.lines() {
                lines.push(Line::from(line.to_owned()));
            }
        }
    } else {
        lines.push(Line::from("Text hidden: debug_log is off."));
    }

    let rows = wrapped_rows(&lines, inner.width as usize);
    let scroll = clamp_scroll(rows, inner.height as usize, scroll);
    let paragraph = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}

/// Rows `lines` take once wrapped at `width` columns (an estimate: one
/// column per character), so scrolling reaches the end of long texts.
fn wrapped_rows(lines: &[Line], width: usize) -> usize {
    let width = width.max(1);
    lines
        .iter()
        .map(|line| line.width().div_ceil(width).max(1))
        .sum()
}

fn clamp_scroll(content_lines: usize, visible_lines: usize, scroll: usize) -> usize {
    if content_lines <= visible_lines {
        0
    } else {
        scroll.min(content_lines - visible_lines)
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let search = if app.search_mode() || !app.search().is_empty() {
        format!(": {}", app.search())
    } else {
        String::new()
    };
    let text = format!(
        "q quit  ↑↓ select  enter details  f filter: {}  / search{}  esc clear",
        app.filter_name(),
        search,
    );
    frame.render_widget(Paragraph::new(text), area);
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
