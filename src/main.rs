use std::fs::{File, OpenOptions};
use std::io::{self, stdout};
use std::time::{Duration, Instant};

use chrono::Local;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use rand::Rng;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

const LOG_FILE: &str = "zetamac_history.csv";
const SPRINT_DURATION_SECS: u64 = 120;

const ZEN_ACCENT: Color = Color::Indexed(110);
const ZEN_MUTED: Color = Color::Indexed(243);
const ZEN_SUCCESS: Color = Color::Indexed(108);

struct TerminalSession {
    active: bool,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let session = Self { active: true };
        execute!(stdout(), EnterAlternateScreen, Hide)?;
        Ok(session)
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }

        self.active = false;
        let raw_mode_result = disable_raw_mode();
        let screen_result = execute!(stdout(), LeaveAlternateScreen, Show);
        raw_mode_result.and(screen_result)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppState {
    StartScreen,
    Running,
    Finished,
}

struct ZetamacZen {
    state: AppState,
    attempt: usize,
    score: usize,
    duration: Duration,
    start_instant: Option<Instant>,
    current_prompt: String,
    current_target: i64,
    input_buffer: String,
    save_error: Option<String>,
}

impl ZetamacZen {
    fn new(attempt: usize) -> Self {
        let mut game = Self {
            state: AppState::StartScreen,
            attempt,
            score: 0,
            duration: Duration::from_secs(SPRINT_DURATION_SECS),
            start_instant: None,
            current_prompt: String::new(),
            current_target: 0,
            input_buffer: String::new(),
            save_error: None,
        };
        game.next_problem();
        game
    }

    fn start(&mut self) {
        self.state = AppState::Running;
        self.start_instant = Some(Instant::now());
        self.score = 0;
        self.input_buffer.clear();
        self.save_error = None;
        self.next_problem();
    }

    fn next_problem(&mut self) {
        let mut rng = rand::rng();
        let ops = ["+", "-", "×", "÷"];
        let op = ops[rng.random_range(0..ops.len())];

        let (prompt, target) = match op {
            "+" => {
                let a = rng.random_range(2..=100);
                let b = rng.random_range(2..=100);
                (format!("{} + {}", a, b), a + b)
            }
            "-" => {
                let a = rng.random_range(2..=100);
                let b = rng.random_range(2..=100);
                (format!("{} - {}", a + b, a), b)
            }
            "×" => {
                let a = rng.random_range(2..=12);
                let b = rng.random_range(2..=100);
                if rng.random_bool(0.5) {
                    (format!("{} × {}", a, b), a * b)
                } else {
                    (format!("{} × {}", b, a), a * b)
                }
            }
            _ => {
                let a = rng.random_range(2..=12);
                let b = rng.random_range(2..=100);
                (format!("{} ÷ {}", a * b, a), b)
            }
        };

        self.current_prompt = prompt;
        self.current_target = target;
        self.input_buffer.clear();
    }

    fn update_timer(&mut self) {
        if self.state == AppState::Running
            && self
                .start_instant
                .is_some_and(|start| sprint_is_over(start, Instant::now(), self.duration))
        {
            self.finish();
        }
    }

    fn finish(&mut self) {
        self.state = AppState::Finished;
        let secs = self.duration.as_secs_f64();
        let rate = if secs > 0.0 {
            (self.score as f64 / secs) * 60.0
        } else {
            0.0
        };
        self.save_error = log_result_csv(self.attempt, self.score, self.duration.as_secs(), rate)
            .err()
            .map(|error| error.to_string());
    }

    fn handle_digit(&mut self, ch: char) {
        self.update_timer();
        if self.state != AppState::Running {
            return;
        }

        self.input_buffer.push(ch);

        if let Ok(parsed) = self.input_buffer.parse::<i64>()
            && parsed == self.current_target
        {
            self.score += 1;
            self.next_problem();
        }
    }

    fn handle_backspace(&mut self) {
        self.input_buffer.pop();
    }
}

fn sprint_is_over(start: Instant, now: Instant, duration: Duration) -> bool {
    now.saturating_duration_since(start) >= duration
}

fn next_attempt_number() -> io::Result<usize> {
    let file = match File::open(LOG_FILE) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(1),
        Err(error) => return Err(error),
    };

    let mut rdr = csv::Reader::from_reader(file);
    let mut count = 0;
    for record in rdr.records() {
        let record = record.map_err(io::Error::other)?;
        let attempt = record
            .get(1)
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid attempt number in history CSV",
                )
            })?;
        count = count.max(attempt);
    }

    count.checked_add(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "attempt number in history CSV is too large",
        )
    })
}

fn log_result_csv(attempt: usize, score: usize, duration: u64, rate: f64) -> io::Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_FILE)?;
    let is_empty = file.metadata()?.len() == 0;

    let mut wtr = csv::Writer::from_writer(file);
    if is_empty {
        wtr.write_record([
            "timestamp",
            "attempt",
            "score",
            "duration_seconds",
            "rate_per_minute",
        ])?;
    }

    let now_str = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    wtr.write_record([
        now_str,
        attempt.to_string(),
        score.to_string(),
        duration.to_string(),
        format!("{:.2}", rate),
    ])?;
    wtr.flush()?;
    Ok(())
}

fn fixed_centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let w = width.min(r.width.saturating_sub(2));
    let h = height.min(r.height.saturating_sub(2));

    let pad_x = (r.width.saturating_sub(w)) / 2;
    let pad_y = (r.height.saturating_sub(h)) / 2;

    Rect {
        x: r.x + pad_x,
        y: r.y + pad_y,
        width: w,
        height: h,
    }
}

fn ui(f: &mut Frame, game: &ZetamacZen) {
    let size = f.area();

    match game.state {
        AppState::StartScreen => {
            let area = fixed_centered_rect(48, 15, size);

            let content = vec![
                Line::from(Span::styled(
                    "zetamac-tui",
                    Style::default().fg(ZEN_ACCENT).bold(),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Attempt : ", Style::default().fg(ZEN_MUTED)),
                    Span::styled(
                        format!("#{}", game.attempt),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " [SPACE] ",
                        Style::default().bg(ZEN_ACCENT).fg(Color::Black).bold(),
                    ),
                    Span::raw("  Begin"),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "[Esc] or [q] to exit",
                    Style::default().fg(ZEN_MUTED),
                )),
            ];

            let p = Paragraph::new(content).alignment(Alignment::Center).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(ZEN_MUTED)),
            );
            f.render_widget(p, area);
        }

        AppState::Running => {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2), // solved counter
                    Constraint::Min(6),    // math card
                    Constraint::Length(2), // exit hint
                ])
                .split(size);

            // Subtle solved counter in the corner
            let counter = Paragraph::new(Line::from(vec![
                Span::styled("solved: ", Style::default().fg(ZEN_MUTED)),
                Span::styled(format!("{}", game.score), Style::default().fg(Color::White)),
                Span::raw("   "),
            ]))
            .alignment(Alignment::Right);
            f.render_widget(counter, layout[0]);

            // Focused, calm math card
            let card_area = fixed_centered_rect(42, 10, layout[1]);

            let card_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(ZEN_ACCENT));

            f.render_widget(card_block, card_area);

            let inner_chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(3), // math expression
                    Constraint::Length(3), // answer input
                ])
                .split(card_area);

            let prompt_text = Paragraph::new(Line::from(Span::styled(
                &game.current_prompt,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center);
            f.render_widget(prompt_text, inner_chunks[0]);

            let input_display = if game.input_buffer.is_empty() {
                "·".to_string()
            } else {
                format!("{}▎", game.input_buffer)
            };

            let input_box = Paragraph::new(Line::from(vec![
                Span::styled("= ", Style::default().fg(ZEN_MUTED)),
                Span::styled(input_display, Style::default().fg(ZEN_SUCCESS).bold()),
            ]))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(Style::default().fg(ZEN_MUTED)),
            );
            f.render_widget(input_box, inner_chunks[1]);

            let footer = Paragraph::new(Span::styled(
                "[Esc] or [q] to stop early",
                Style::default().fg(ZEN_MUTED),
            ))
            .alignment(Alignment::Center);
            f.render_widget(footer, layout[2]);
        }

        AppState::Finished => {
            let area = fixed_centered_rect(50, 16, size);
            let secs = game.duration.as_secs_f64();
            let rate = if secs > 0.0 {
                (game.score as f64 / secs) * 60.0
            } else {
                0.0
            };
            let save_status = game.save_error.as_ref().map_or_else(
                || format!("Saved to {LOG_FILE}"),
                |error| format!("Save failed: {error}"),
            );
            let save_style = if game.save_error.is_some() {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(ZEN_MUTED)
            };

            let summary = vec![
                Line::from(Span::styled(
                    "s e s s i o n   c o m p l e t e",
                    Style::default().fg(ZEN_ACCENT).bold(),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Attempt  : ", Style::default().fg(ZEN_MUTED)),
                    Span::styled(
                        format!("#{}", game.attempt),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Solved   : ", Style::default().fg(ZEN_MUTED)),
                    Span::styled(
                        format!("{} equations", game.score),
                        Style::default().fg(ZEN_SUCCESS).bold(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Pace     : ", Style::default().fg(ZEN_MUTED)),
                    Span::styled(
                        format!("{:.1} / min", rate),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(save_status, save_style)),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " [SPACE] ",
                        Style::default().bg(ZEN_ACCENT).fg(Color::Black).bold(),
                    ),
                    Span::raw("  Another session"),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "[Esc] or [q] to exit",
                    Style::default().fg(ZEN_MUTED),
                )),
            ];

            let p = Paragraph::new(summary).alignment(Alignment::Center).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(ZEN_MUTED)),
            );
            f.render_widget(p, area);
        }
    }
}

fn main() -> io::Result<()> {
    let mut session = TerminalSession::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut game = ZetamacZen::new(next_attempt_number()?);
    let tick_rate = Duration::from_millis(25);

    loop {
        game.update_timer();
        terminal.draw(|f| ui(f, &game))?;

        if event::poll(tick_rate)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            let state_before_key = game.state;
            if state_before_key == AppState::Running {
                game.update_timer();
            }

            match state_before_key {
                AppState::StartScreen => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char(' ') | KeyCode::Enter => game.start(),
                    _ => {}
                },

                AppState::Running => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char(c) if game.state == AppState::Running && c.is_ascii_digit() => {
                        game.handle_digit(c)
                    }
                    KeyCode::Backspace if game.state == AppState::Running => {
                        game.handle_backspace()
                    }
                    _ => {}
                },

                AppState::Finished => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        game = ZetamacZen::new(next_attempt_number()?);
                        game.start();
                    }
                    _ => {}
                },
            }
        }
    }

    drop(terminal);
    session.restore()
}
