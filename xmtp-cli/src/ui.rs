//! TUI rendering — header, sidebar, chat (with scroll), input, status, help.

use std::time::SystemTime;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use xmtp::MessageKind;

use crate::app::{message_body, truncate_id, App, Focus, Mode};

// ── Palette ──────────────────────────────────────────────────────────────────

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const SENDER_ME: Color = Color::Green;
const SENDER_PEER: Color = Color::Cyan;
const UNREAD_DOT: Color = Color::Yellow;
const GROUP_TAG: Color = Color::Magenta;

// ── Root ─────────────────────────────────────────────────────────────────────

/// Render the full application UI.
pub fn render(app: &App, frame: &mut Frame<'_>) {
    let area = frame.area();

    // Vertical split: header(1) | body(fill) | status(1)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(app, frame, rows[0]);
    draw_body(app, frame, rows[1]);
    draw_status(app, frame, rows[2]);

    // Overlay layer (drawn last so it sits on top).
    if app.mode == Mode::Help {
        draw_help_overlay(frame, area);
    }
}

// ── Header ───────────────────────────────────────────────────────────────────

fn draw_header(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let inbox_short = truncate_id(&app.my_inbox_id, 16);
    let line = Line::from(vec![
        Span::styled(" XMTP ", Style::default().fg(Color::Black).bg(ACCENT)),
        Span::raw("  "),
        Span::styled(&app.name, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("  ·  dev  ·  ", Style::default().fg(DIM)),
        Span::styled(inbox_short, Style::default().fg(DIM)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ── Body (sidebar + main) ───────────────────────────────────────────────────

fn draw_body(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(30)])
        .split(area);

    draw_sidebar(app, frame, cols[0]);

    // Main column: chat(fill) | input(3)
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(cols[1]);

    draw_chat(app, frame, main[0]);
    draw_input(app, frame, main[1]);
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

fn draw_sidebar(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let focused = app.focus == Focus::Sidebar && app.mode == Mode::Normal;
    let border_style = Style::default().fg(if focused { ACCENT } else { DIM });
    let block = Block::default()
        .title(" Chats ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if app.conversations.is_empty() {
        let hint = Paragraph::new("\n  No conversations\n\n  Press  n  to start")
            .style(Style::default().fg(DIM))
            .block(block);
        frame.render_widget(hint, area);
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .conversations
        .iter()
        .map(|c| {
            // Row 1: [●] label [time]
            let dot = if c.unread {
                Span::styled("● ", Style::default().fg(UNREAD_DOT))
            } else {
                Span::raw("  ")
            };
            let tag = if c.is_group {
                Span::styled("⊞ ", Style::default().fg(GROUP_TAG))
            } else {
                Span::raw("")
            };
            let time_str = if c.last_ns > 0 {
                format_relative(c.last_ns)
            } else {
                String::new()
            };
            let row1 = Line::from(vec![
                dot,
                tag,
                Span::styled(
                    c.label.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {time_str}"), Style::default().fg(DIM)),
            ]);
            // Row 2: preview
            let row2 = Line::from(vec![
                Span::raw("  "),
                Span::styled(c.preview.clone(), Style::default().fg(DIM)),
            ]);
            ListItem::new(vec![row1, row2])
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▸ ");

    let mut state = ListState::default().with_selected(Some(app.sidebar_idx));
    frame.render_stateful_widget(list, area, &mut state);
}

// ── Chat area ────────────────────────────────────────────────────────────────

fn draw_chat(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::TOP | Borders::RIGHT)
        .border_style(Style::default().fg(DIM));
    let inner = block.inner(area);

    if app.active_conv_id.is_none() {
        let welcome = Paragraph::new(Text::from(vec![
            Line::default(),
            Line::from("  Welcome to XMTP Chat"),
            Line::default(),
            Line::from(vec![
                Span::styled("  Select a conversation or press ", Style::default().fg(DIM)),
                Span::styled("n", Style::default().fg(ACCENT)),
                Span::styled(" to start a new DM", Style::default().fg(DIM)),
            ]),
        ]))
        .block(block);
        frame.render_widget(welcome, area);
        return;
    }

    // Build all rendered lines from messages.
    let mut lines: Vec<Line<'_>> = Vec::new();
    for msg in &app.messages {
        if msg.kind != MessageKind::Application {
            continue;
        }
        let is_me = msg.sender_inbox_id == app.my_inbox_id;
        let sender = if is_me {
            "you".to_owned()
        } else {
            truncate_id(&msg.sender_inbox_id, 12)
        };
        let time = format_relative(msg.sent_at_ns);
        let body = message_body(msg);

        let name_style = if is_me {
            Style::default().fg(SENDER_ME)
        } else {
            Style::default().fg(SENDER_PEER)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {sender}"), name_style),
            Span::styled(format!("  {time}"), Style::default().fg(DIM)),
        ]));
        for text_line in body.lines() {
            lines.push(Line::from(format!("    {text_line}")));
        }
        lines.push(Line::default());
    }

    // Apply scroll: offset 0 = bottom (latest), offset N = N lines up.
    let view_h = inner.height as usize;
    let total = lines.len();
    let max_offset = total.saturating_sub(view_h);
    let offset = app.scroll_offset.min(max_offset);
    let start = total.saturating_sub(view_h + offset);
    let end = total.saturating_sub(offset);
    let visible: Vec<Line<'_>> = lines.into_iter().skip(start).take(end - start).collect();

    let paragraph = Paragraph::new(Text::from(visible))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);

    // Scroll indicator when not at bottom.
    if offset > 0 {
        let indicator = format!(" ↑{offset} ");
        #[allow(clippy::cast_possible_truncation)]
        let x = area.x + area.width.saturating_sub(indicator.len() as u16 + 2);
        let y = area.y;
        frame.render_widget(
            Paragraph::new(Span::styled(indicator, Style::default().fg(ACCENT))),
            Rect::new(x, y, 10, 1),
        );
    }
}

// ── Input bar ────────────────────────────────────────────────────────────────

fn draw_input(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let focused = app.focus == Focus::Input && app.mode == Mode::Normal;
    let border_color = if focused { ACCENT } else { DIM };

    let title = match app.mode {
        Mode::NewDm => " New DM ",
        _ => "",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let prompt = match app.mode {
        Mode::NewDm => "inbox> ",
        Mode::Normal | Mode::Help => "> ",
    };
    let display = format!("{prompt}{}", app.input);
    let paragraph = Paragraph::new(display).block(block);
    frame.render_widget(paragraph, area);

    // Cursor positioning (Unicode-width-aware).
    if focused || app.mode == Mode::NewDm {
        let prefix_before_cursor: String = app.input.chars().take(app.input_cursor).collect();
        let visual_offset = UnicodeWidthStr::width(prefix_before_cursor.as_str());
        #[allow(clippy::cast_possible_truncation)]
        let x = area.x + 1 + prompt.len() as u16 + visual_offset as u16;
        let y = area.y + 1;
        frame.set_cursor_position((x, y));
    }
}

// ── Status bar ───────────────────────────────────────────────────────────────

fn draw_status(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let line = Line::from(vec![
        Span::styled(&app.status, Style::default().fg(DIM)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ── Help overlay ─────────────────────────────────────────────────────────────

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect) {
    let w = 44.min(area.width.saturating_sub(4));
    let h = 18.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    let block = Block::default()
        .title(" Keyboard Shortcuts ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));

    let help_text = Text::from(vec![
        Line::default(),
        help_line("Tab", "Switch sidebar / input"),
        help_line("j / k", "Navigate conversations"),
        help_line("g / G", "Jump to first / last"),
        help_line("Enter", "Open conversation / send"),
        help_line("l / →", "Focus input"),
        help_line("n", "New DM"),
        help_line("r", "Sync conversations"),
        help_line("PgUp/PgDn", "Scroll chat"),
        help_line("Esc", "Cancel / back"),
        help_line("q", "Quit"),
        help_line("Ctrl-C", "Force quit"),
        Line::default(),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(DIM),
        )),
    ]);

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(help_text).block(block), popup);
}

fn help_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {key:>12}  "), Style::default().fg(ACCENT)),
        Span::raw(desc),
    ])
}

// ── Time formatting ──────────────────────────────────────────────────────────

/// Format a nanosecond timestamp as relative time (e.g. `now`, `3m`, `2h`, `5d`).
#[allow(clippy::cast_possible_truncation)]
fn format_relative(ns: i64) -> String {
    let now_ns = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    let secs = (now_ns - ns) / 1_000_000_000;
    if secs < 60 {
        return "now".into();
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86400 {
        return format!("{}h", secs / 3600);
    }
    format!("{}d", secs / 86400)
}
