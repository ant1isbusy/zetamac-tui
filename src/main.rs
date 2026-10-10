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
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

const LOG_FILE: &str = "zetamac_history.csv";
const SPRINT_DURATION_SECS: u64 = 120;

const THEME_ACCENT: Color = Color::Rgb(136, 192, 208);
const THEME_MUTED: Color = Color::Rgb(76, 86, 106);
const THEME_SUBTLE: Color = Color::Rgb(140, 150, 170);
const THEME_GREEN: Color = Color::Rgb(163, 190, 140);
const THEME_YELLOW: Color = Color::Rgb(235, 203, 139);

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

struct Zetamac {
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

impl Zetamac {
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

    fn remaining_secs(&self) -> u64 {
        if let Some(start) = self.start_instant {
            let elapsed = Instant::now().saturating_duration_since(start);
            self.duration.saturating_sub(elapsed).as_secs()
        } else {
            self.duration.as_secs()
        }
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
            "equations_per_second",
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

fn ui(f: &mut Frame, game: &Zetamac) {
    let size = f.area();

    match game.state {
        AppState::StartScreen => {
            let card_area = fixed_centered_rect(44, 13, size);

            let content = vec![
                Line::from(vec![
                    Span::styled("Z E T A M A C", Style::default().fg(THEME_ACCENT).bold()),
                    Span::styled(" tui", Style::default().fg(THEME_SUBTLE)),
                ])
                .alignment(Alignment::Center),
                Line::from(Span::styled(
                    "─".repeat(34),
                    Style::default().fg(THEME_MUTED),
                ))
                .alignment(Alignment::Center),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Attempt   : ", Style::default().fg(THEME_SUBTLE)),
                    Span::styled(
                        format!("#{}", game.attempt),
                        Style::default().fg(Color::White).bold(),
                    ),
                ])
                .alignment(Alignment::Center),
                Line::from(vec![
                    Span::styled("Duration  : ", Style::default().fg(THEME_SUBTLE)),
                    Span::styled(
                        format!("{}s", SPRINT_DURATION_SECS),
                        Style::default().fg(Color::White),
                    ),
                ])
                .alignment(Alignment::Center),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " [SPACE] ",
                        Style::default().bg(THEME_ACCENT).fg(Color::Black).bold(),
                    ),
                    Span::styled("  Start sprint", Style::default().fg(Color::White)),
                ])
                .alignment(Alignment::Center),
                Line::from(""),
                Line::from(Span::styled(
                    "[Esc] or [q] to exit",
                    Style::default().fg(THEME_MUTED),
                ))
                .alignment(Alignment::Center),
            ];

            let p = Paragraph::new(content).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(THEME_MUTED)),
            );
            f.render_widget(p, card_area);
        }

        AppState::Running => {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Fill(1),
                    Constraint::Length(2),
                ])
                .split(size);

            let rem_secs = game.remaining_secs();
            let time_color = if rem_secs <= 10 {
                Color::LightRed
            } else if rem_secs <= 30 {
                THEME_YELLOW
            } else {
                THEME_ACCENT
            };

            let header_line = Line::from(vec![
                Span::styled("TIME ", Style::default().fg(THEME_MUTED).bold()),
                Span::styled(
                    format!("{:>3}s", rem_secs),
                    Style::default().fg(time_color).bold(),
                ),
                Span::styled("  │  ", Style::default().fg(THEME_MUTED)),
                Span::styled("SCORE ", Style::default().fg(THEME_MUTED).bold()),
                Span::styled(
                    format!("{:<2}", game.score),
                    Style::default().fg(THEME_GREEN).bold(),
                ),
            ])
            .alignment(Alignment::Center);

            let header_bar = Paragraph::new(header_line)
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(THEME_MUTED)),
                );
            f.render_widget(header_bar, layout[0]);

            let input_len = game.input_buffer.len();
            let padding_spaces = 3usize.saturating_sub(input_len);

            let math_lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        &game.current_prompt,
                        Style::default().fg(Color::White).bold(),
                    ),
                    Span::styled("  =  ", Style::default().fg(THEME_MUTED).bold()),
                    Span::styled(&game.input_buffer, Style::default().fg(THEME_YELLOW).bold()),
                    Span::styled("▌", Style::default().fg(THEME_ACCENT)),
                    Span::raw(" ".repeat(padding_spaces)),
                ])
                .alignment(Alignment::Center),
            ];

            let math_card = Paragraph::new(math_lines).alignment(Alignment::Center);
            f.render_widget(math_card, layout[1]);
            let footer = Paragraph::new(Line::from(vec![
                Span::styled("esc", Style::default().fg(THEME_SUBTLE)),
                Span::styled("  abandon sprint", Style::default().fg(THEME_MUTED)),
            ]))
            .alignment(Alignment::Center);
            f.render_widget(footer, layout[2]);
        }
        AppState::Finished => {
            let area = fixed_centered_rect(44, 15, size);
            let secs = game.duration.as_secs_f64();
            let rate = if game.score > 0 {
                secs / game.score as f64
            } else {
                0.0
            };

            let save_msg = if let Some(ref err) = game.save_error {
                Span::styled(format!("error: {err}"), Style::default().fg(Color::Red))
            } else {
                Span::styled(
                    format!("saved → {LOG_FILE}"),
                    Style::default().fg(THEME_MUTED),
                )
            };

            let summary = vec![
                Line::from(vec![
                    Span::styled("COMPLETE", Style::default().fg(THEME_GREEN).bold()),
                    Span::styled(" //", Style::default().fg(THEME_MUTED)),
                    Span::styled(" summary", Style::default().fg(THEME_SUBTLE)),
                ])
                .alignment(Alignment::Center),
                Line::from(Span::styled(
                    "─".repeat(34),
                    Style::default().fg(THEME_MUTED),
                ))
                .alignment(Alignment::Center),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Attempt    : ", Style::default().fg(THEME_SUBTLE)),
                    Span::styled(
                        format!("#{}", game.attempt),
                        Style::default().fg(Color::White),
                    ),
                ])
                .alignment(Alignment::Center),
                Line::from(vec![
                    Span::styled("Solved     : ", Style::default().fg(THEME_SUBTLE)),
                    Span::styled(
                        format!("{} eq.", game.score),
                        Style::default().fg(THEME_GREEN).bold(),
                    ),
                ])
                .alignment(Alignment::Center),
                Line::from(vec![
                    Span::styled("Avg Pace   : ", Style::default().fg(THEME_SUBTLE)),
                    Span::styled(
                        format!("{:.2}s / eq.", rate),
                        Style::default().fg(Color::White),
                    ),
                ])
                .alignment(Alignment::Center),
                Line::from(""),
                Line::from(save_msg).alignment(Alignment::Center),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " [SPACE] ",
                        Style::default().bg(THEME_ACCENT).fg(Color::Black).bold(),
                    ),
                    Span::styled(" Retry   ", Style::default().fg(Color::White)),
                    Span::styled(" [Esc] ", Style::default().fg(THEME_MUTED)),
                    Span::styled("Quit", Style::default().fg(THEME_SUBTLE)),
                ])
                .alignment(Alignment::Center),
            ];

            let p = Paragraph::new(summary).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(THEME_MUTED)),
            );
            f.render_widget(p, area);
        }
    }
}

fn main() -> io::Result<()> {
    let mut session = TerminalSession::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut game = Zetamac::new(next_attempt_number()?);
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
                        game = Zetamac::new(next_attempt_number()?);
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
