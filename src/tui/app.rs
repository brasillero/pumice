//! Pure state of the live request view (S4.6). No terminal code.
//!
//! [`App`] collects monitor events, filters and searches them, tracks which
//! request is selected and whether the details pane is open. The [`view`]
//! module renders this state; tests drive it directly.
//!
//! [`view`]: super::view

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::monitor::{Event, EventKind};
use crate::pipeline::{AttemptResult, OutcomeKind};
use crate::providers::diagnostic::Diagnostic;

use super::Info;

/// Lines one PageUp/PageDown moves the details pane.
const DETAILS_PAGE: usize = 5;

/// Most requests kept; the oldest finished ones go first.
const MAX_REQUESTS: usize = 500;

/// Everything known about one request as displayed in the live view.
pub struct RequestView {
    pub number: u64,
    pub arrived: Instant,
    pub arrived_wall: jiff::Zoned,
    /// When the body was read and parsed.
    pub parsed: Option<Instant>,
    pub model: Option<String>,
    pub input: Option<String>,
    pub queued: Option<Instant>,
    pub started: Option<Started>,
    pub attempt: Option<Attempt>,
    pub responded: Option<Responded>,
    pub dropped: Option<Instant>,
}

/// When the provider CLI started and which one.
pub struct Started {
    pub at: Instant,
    pub provider: &'static str,
    pub model: String,
}

/// When the provider run and cleanup ended.
pub struct Attempt {
    pub at: Instant,
    pub result: AttemptResult,
    pub diagnostic: Option<Diagnostic>,
}

/// When and how the response was sent.
pub struct Responded {
    pub at: Instant,
    pub status: u16,
    pub outcome: Option<OutcomeKind>,
    pub detail: String,
    pub reply: Option<String>,
}

/// Derived lifecycle state shown in the table.
pub enum State {
    Reading,
    Queued,
    Running(&'static str),
    Sending,
    Done(u16),
    Failed(u16),
    Dropped,
}

impl RequestView {
    /// Current lifecycle state.
    pub fn state(&self) -> State {
        if self.dropped.is_some() {
            return State::Dropped;
        }
        if let Some(resp) = &self.responded {
            return if resp.status >= 400 {
                State::Failed(resp.status)
            } else {
                State::Done(resp.status)
            };
        }
        if self.attempt.is_some() {
            return State::Sending;
        }
        if let Some(started) = &self.started {
            return State::Running(started.provider);
        }
        if self.queued.is_some() {
            return State::Queued;
        }
        State::Reading
    }

    /// Time spent in line for a free slot, if the request ever queued: until
    /// its CLI started, or until it got an answer or went away without one.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let queued = self.queued?;
        let end = self
            .started
            .as_ref()
            .map(|started| started.at)
            .or(self.responded.as_ref().map(|responded| responded.at))
            .or(self.dropped)
            .unwrap_or(now);
        Some(end.duration_since(queued))
    }

    /// Time spent inside the provider CLI, if it started: until the run
    /// ended, or until the request got an answer or went away (which
    /// cancels the run).
    pub fn cli(&self, now: Instant) -> Option<Duration> {
        let started = self.started.as_ref()?;
        let end = self
            .attempt
            .as_ref()
            .map(|attempt| attempt.at)
            .or(self.responded.as_ref().map(|responded| responded.at))
            .or(self.dropped)
            .unwrap_or(now);
        Some(end.duration_since(started.at))
    }

    /// Whether the request got an answer or went away.
    pub fn finished(&self) -> bool {
        self.responded.is_some() || self.dropped.is_some()
    }

    /// Whether the request is still waiting in line.
    pub fn waiting_in_line(&self) -> bool {
        self.queued.is_some() && self.started.is_none() && !self.finished()
    }

    /// Whether its CLI is still running.
    pub fn cli_running(&self) -> bool {
        self.started.is_some() && self.attempt.is_none() && !self.finished()
    }

    /// Total time since the request arrived.
    pub fn total(&self, now: Instant) -> Option<Duration> {
        if let Some(dropped) = self.dropped {
            Some(dropped.duration_since(self.arrived))
        } else if let Some(responded) = &self.responded {
            Some(responded.at.duration_since(self.arrived))
        } else {
            Some(now.duration_since(self.arrived))
        }
    }
}

/// Filter applied to the request list.
#[derive(Clone)]
enum Filter {
    All,
    Failed,
    Model(String),
}

/// Action returned by key handling for the terminal loop to perform.
pub enum Action {
    /// Nothing special; keep looping.
    None,
    /// Start graceful shutdown (first press) or exit immediately (second).
    Quit,
}

/// State of the live request view.
pub struct App {
    requests: Vec<RequestView>,
    selection: Option<u64>,
    /// Whether the selection tracks the newest visible request.
    follow: bool,
    filter: Filter,
    search: String,
    search_mode: bool,
    details: bool,
    details_scroll: usize,
    stopping: bool,
    info: Info,
}

impl App {
    /// Creates an empty app with the given startup information.
    pub fn new(info: Info) -> App {
        App {
            requests: Vec::new(),
            selection: None,
            follow: true,
            filter: Filter::All,
            search: String::new(),
            search_mode: false,
            details: false,
            details_scroll: 0,
            stopping: false,
            info,
        }
    }

    /// Returns the startup/info block.
    pub fn info(&self) -> &Info {
        &self.info
    }

    /// Whether the details pane is open.
    pub fn details_open(&self) -> bool {
        self.details
    }

    /// Current scroll offset of the details pane.
    pub fn details_scroll(&self) -> usize {
        self.details_scroll
    }

    /// Whether a key press is currently editing the search query.
    pub fn search_mode(&self) -> bool {
        self.search_mode
    }

    /// Current search query.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Whether the user asked to stop the service.
    pub fn stopping(&self) -> bool {
        self.stopping
    }

    /// Sets the stopping flag so the header shows the drain state.
    pub fn set_stopping(&mut self) {
        self.stopping = true;
    }

    /// Number of unfinished requests still in the app.
    pub fn waiting(&self) -> usize {
        self.requests.iter().filter(|r| !r.finished()).count()
    }

    /// Applies one monitor event, updating or creating the request it belongs to.
    pub fn apply(&mut self, event: Event) {
        match event.kind {
            EventKind::Arrived => {
                let view = RequestView {
                    number: event.number,
                    arrived: event.at,
                    arrived_wall: jiff::Zoned::now(),
                    parsed: None,
                    model: None,
                    input: None,
                    queued: None,
                    started: None,
                    attempt: None,
                    responded: None,
                    dropped: None,
                };
                self.requests.insert(0, view);
            }
            EventKind::Parsed { model, input } => {
                if let Some(req) = self.request_mut(event.number) {
                    req.parsed = Some(event.at);
                    req.model = model;
                    req.input = input;
                }
            }
            EventKind::Queued => {
                if let Some(req) = self.request_mut(event.number) {
                    req.queued = Some(event.at);
                }
            }
            EventKind::Started { provider, model } => {
                if let Some(req) = self.request_mut(event.number) {
                    req.started = Some(Started {
                        at: event.at,
                        provider,
                        model,
                    });
                }
            }
            EventKind::AttemptEnded { result, diagnostic } => {
                if let Some(req) = self.request_mut(event.number) {
                    req.attempt = Some(Attempt {
                        at: event.at,
                        result,
                        diagnostic,
                    });
                }
            }
            EventKind::Responded {
                status,
                outcome,
                detail,
                reply,
            } => {
                if let Some(req) = self.request_mut(event.number) {
                    req.responded = Some(Responded {
                        at: event.at,
                        status,
                        outcome,
                        detail,
                        reply,
                    });
                }
            }
            EventKind::Dropped => {
                if let Some(req) = self.request_mut(event.number) {
                    req.dropped = Some(event.at);
                }
            }
        }
        self.trim_finished();
        self.sync_selection();
    }

    /// Handles one keyboard event.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            return Action::Quit;
        }
        if self.search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.search.clear();
                    self.search_mode = false;
                }
                KeyCode::Enter => self.search_mode = false,
                KeyCode::Backspace => {
                    self.search.pop();
                }
                KeyCode::Char(c) => self.search.push(c),
                _ => {}
            }
            self.sync_selection();
            return Action::None;
        }
        match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                Action::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.select_first();
                Action::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.select_last();
                Action::None
            }
            KeyCode::Enter => {
                self.details = !self.details;
                self.details_scroll = 0;
                Action::None
            }
            KeyCode::PageUp => {
                self.details_scroll = self.details_scroll.saturating_sub(DETAILS_PAGE);
                Action::None
            }
            KeyCode::PageDown => {
                self.details_scroll = self.details_scroll.saturating_add(DETAILS_PAGE);
                Action::None
            }
            KeyCode::Char('f') => {
                self.next_filter();
                self.sync_selection();
                Action::None
            }
            KeyCode::Char('/') => {
                self.search_mode = true;
                Action::None
            }
            KeyCode::Esc => {
                self.filter = Filter::All;
                self.search.clear();
                self.sync_selection();
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Requests after filter and search, newest first.
    pub fn rows(&self) -> Vec<&RequestView> {
        self.requests.iter().filter(|r| self.matches(r)).collect()
    }

    /// The currently selected request, if any.
    pub fn selected(&self) -> Option<&RequestView> {
        let rows = self.rows();
        rows.into_iter().find(|r| Some(r.number) == self.selection)
    }

    /// Number of provider runs currently active.
    pub fn running(&self) -> usize {
        self.requests.iter().filter(|r| r.cli_running()).count()
    }

    /// Number of requests queued and not yet started.
    pub fn queued(&self) -> usize {
        self.requests.iter().filter(|r| r.waiting_in_line()).count()
    }

    /// Number of completed 2xx responses.
    pub fn ok(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| matches!(r.responded.as_ref().map(|r| r.status), Some(s) if s < 400))
            .count()
    }

    /// Number of completed error (>=400) responses.
    pub fn failed(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| matches!(r.responded.as_ref().map(|r| r.status), Some(s) if s >= 400))
            .count()
    }

    /// Number of dropped requests.
    pub fn dropped(&self) -> usize {
        self.requests.iter().filter(|r| r.dropped.is_some()).count()
    }

    /// Per-provider count and average CLI time, in first-seen order.
    pub fn provider_stats(&self) -> Vec<(&'static str, usize, Duration)> {
        let mut order: Vec<&'static str> = Vec::new();
        let mut totals: std::collections::HashMap<&'static str, (usize, Duration)> =
            std::collections::HashMap::new();
        for req in &self.requests {
            let Some(attempt) = &req.attempt else {
                continue;
            };
            let Some(started) = &req.started else {
                continue;
            };
            let provider = started.provider;
            let entry = totals.entry(provider).or_insert((0, Duration::ZERO));
            entry.0 += 1;
            entry.1 += attempt.at.duration_since(started.at);
            if !order.contains(&provider) {
                order.push(provider);
            }
        }
        order
            .into_iter()
            .map(|provider| {
                let (count, total) = totals[&provider];
                let avg = if count > 0 {
                    total / count as u32
                } else {
                    Duration::ZERO
                };
                (provider, count, avg)
            })
            .collect()
    }

    /// Human-readable name of the active filter.
    pub fn filter_name(&self) -> String {
        match &self.filter {
            Filter::All => "all".to_owned(),
            Filter::Failed => "failed".to_owned(),
            Filter::Model(model) => format!("model:{}", model.trim()),
        }
    }

    fn request_mut(&mut self, number: u64) -> Option<&mut RequestView> {
        self.requests.iter_mut().find(|r| r.number == number)
    }

    /// Drops the oldest finished requests beyond [`MAX_REQUESTS`]; requests
    /// still in progress are always kept.
    fn trim_finished(&mut self) {
        while self.requests.len() > MAX_REQUESTS {
            let Some(oldest_finished) = self.requests.iter().rposition(RequestView::finished)
            else {
                break;
            };
            self.requests.remove(oldest_finished);
        }
    }

    /// Keeps the selection on a visible row: the newest one while following,
    /// otherwise the same request if it is still visible, else the newest.
    fn sync_selection(&mut self) {
        let rows = self.rows();
        let newest = rows.first().map(|r| r.number);
        let visible = rows.iter().any(|r| Some(r.number) == self.selection);
        let selection = if self.follow || !visible {
            newest
        } else {
            self.selection
        };
        self.selection = selection;
        self.follow = selection == newest;
    }

    fn move_selection(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let current = rows
            .iter()
            .position(|r| Some(r.number) == self.selection)
            .unwrap_or(0);
        let new_index = (current as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.selection = Some(rows[new_index].number);
        self.follow = new_index == 0;
    }

    fn select_first(&mut self) {
        if let Some(first) = self.rows().first() {
            self.selection = Some(first.number);
            self.follow = true;
        }
    }

    fn select_last(&mut self) {
        let rows = self.rows();
        if let Some(last) = rows.last() {
            let (number, only) = (last.number, rows.len() == 1);
            self.selection = Some(number);
            self.follow = only;
        }
    }

    fn next_filter(&mut self) {
        let models = self.models_first_seen();
        self.filter = match &self.filter {
            Filter::All => Filter::Failed,
            Filter::Failed => models
                .first()
                .cloned()
                .map(Filter::Model)
                .unwrap_or(Filter::All),
            Filter::Model(model) => {
                let current = model.trim().to_lowercase();
                let pos = models
                    .iter()
                    .position(|m| m.trim().to_lowercase() == current);
                match pos {
                    Some(i) if i + 1 < models.len() => Filter::Model(models[i + 1].clone()),
                    _ => Filter::All,
                }
            }
        }
    }

    /// Requested models, oldest request first (`requests` is newest first).
    fn models_first_seen(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut models = Vec::new();
        for req in self.requests.iter().rev() {
            if let Some(model) = &req.model {
                let key = model.trim().to_lowercase();
                if seen.insert(key) {
                    models.push(model.clone());
                }
            }
        }
        models
    }

    fn matches(&self, req: &RequestView) -> bool {
        if !self.matches_filter(req) {
            return false;
        }
        if self.search.is_empty() {
            return true;
        }
        let query = self.search.to_lowercase();
        if req.number.to_string().contains(&query) {
            return true;
        }
        if req
            .model
            .as_ref()
            .map(|m| m.to_lowercase().contains(&query))
            .unwrap_or(false)
        {
            return true;
        }
        if let Some(started) = &req.started
            && started.provider.to_lowercase().contains(&query)
        {
            return true;
        }
        if let Some(resp) = &req.responded
            && resp.detail.to_lowercase().contains(&query)
        {
            return true;
        }
        if self.info.text_allowed {
            if req
                .input
                .as_ref()
                .map(|s| s.to_lowercase().contains(&query))
                .unwrap_or(false)
            {
                return true;
            }
            if req
                .responded
                .as_ref()
                .and_then(|r| r.reply.as_ref())
                .map(|s| s.to_lowercase().contains(&query))
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    fn matches_filter(&self, req: &RequestView) -> bool {
        match &self.filter {
            Filter::All => true,
            Filter::Failed => matches!(req.state(), State::Failed(_) | State::Dropped),
            Filter::Model(model) => req
                .model
                .as_ref()
                .map(|m| m.trim().eq_ignore_ascii_case(model.trim()))
                .unwrap_or(false),
        }
    }
}
