//! TUI rendering: header, tabbed sidebar, chat bubbles, input, overlays.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use xmtp::MessageKind;

use crate::app::{App, Focus, Mode, Tab, decode_body, delivery_icon, truncate_id};

// ── Palette ──────────────────────────────────────────────────────

const ACCENT: Color = Color::Blue;
const DIM: Color = Color::DarkGray;
const SELF_BG: Color = Color::Green;
const PEER_BG: Color = Color::Cyan;
const UNREAD: Color = Color::Yellow;
const GROUP_TAG: Color = Color::Magenta;
const TAB_ACTIVE: Color = Color::White;
const TAB_INACTIVE: Color = Color::DarkGray;
const REQUEST_TAG: Color = Color::Yellow;

// ── Root ─────────────────────────────────────────────────────────

/// Render the full application UI.
pub fn render(app: &App, frame: &mut Frame<'_>) {
    let area = frame.area();
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

    // Overlays
    match app.mode {
        Mode::Help => draw_help(frame, area),
        Mode::Members => draw_members(app, frame, area),
        _ => {}
    }
}

// ── Header ───────────────────────────────────────────────────────

fn draw_header(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let req_count = app.requests.len();
    let mut spans = vec![
        Span::styled(" XMTP ", Style::default().fg(Color::Black).bg(ACCENT)),
        Span::raw("  "),
        Span::styled(&app.address, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("  ·  dev  ", Style::default().fg(DIM)),
    ];
    if req_count > 0 {
        spans.push(Span::styled(
            format!("·  {req_count} request(s)  "),
            Style::default().fg(REQUEST_TAG),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Body (sidebar + main) ───────────────────────────────────────

fn draw_body(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let sidebar_w = (area.width * 3 / 10).clamp(24, 38);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_w), Constraint::Min(30)])
        .split(area);

    draw_sidebar(app, frame, cols[0]);

    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(cols[1]);

    draw_chat(app, frame, main[0]);
    draw_input(app, frame, main[1]);
}

// ── Sidebar with tabs ────────────────────────────────────────────

fn draw_sidebar(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let focused = app.focus == Focus::Sidebar && app.mode == Mode::Normal;
    let border = Style::default().fg(if focused { ACCENT } else { DIM });

    // Tab header: [1:Inbox] [2:Requests]
    let req_label = format!(" 2:Requests({}) ", app.requests.len());
    let tab_line = Line::from(vec![
        tab_span(" 1:Inbox ", app.tab == Tab::Inbox),
        Span::raw(" "),
        tab_span(&req_label, app.tab == Tab::Requests),
    ]);

    let block = Block::default()
        .title(tab_line)
        .borders(Borders::ALL)
        .border_style(border);

    let list_data = app.sidebar();

    if list_data.is_empty() {
        let hint = match app.tab {
            Tab::Inbox => "\n  No conversations\n\n  Press  n  for DM\n  Press  g  for group",
            Tab::Requests => "\n  No pending requests",
        };
        let p = Paragraph::new(hint)
            .style(Style::default().fg(DIM))
            .block(block);
        frame.render_widget(p, area);
        return;
    }

    let items: Vec<ListItem<'_>> = list_data
        .iter()
        .map(|c| {
            let dot = if c.unread {
                Span::styled("● ", Style::default().fg(UNREAD))
            } else {
                Span::raw("  ")
            };
            let tag = if c.is_group {
                Span::styled("⊞ ", Style::default().fg(GROUP_TAG))
            } else {
                Span::raw("")
            };
            let time = if c.last_ns > 0 {
                format_relative(c.last_ns)
            } else {
                String::new()
            };
            let row1 = Line::from(vec![
                dot,
                tag,
                Span::styled(&c.label, Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {time}"), Style::default().fg(DIM)),
            ]);
            let row2 = Line::from(vec![
                Span::raw("  "),
                Span::styled(&c.preview, Style::default().fg(DIM)),
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

fn tab_span(label: &str, active: bool) -> Span<'_> {
    if active {
        Span::styled(
            label.to_owned(),
            Style::default().fg(TAB_ACTIVE).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label.to_owned(), Style::default().fg(TAB_INACTIVE))
    }
}

// ── Chat area (bubble messages) ─────────────────────────────────

fn draw_chat(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::TOP | Borders::RIGHT)
        .border_style(Style::default().fg(DIM));
    let inner = block.inner(area);

    if app.active_id.is_none() {
        let welcome = Paragraph::new(Text::from(vec![
            Line::default(),
            Line::from("  Welcome to XMTP Chat"),
            Line::default(),
            Line::from(vec![
                Span::styled("  Press ", Style::default().fg(DIM)),
                Span::styled("n", Style::default().fg(ACCENT)),
                Span::styled(" for DM · ", Style::default().fg(DIM)),
                Span::styled("g", Style::default().fg(ACCENT)),
                Span::styled(" for group", Style::default().fg(DIM)),
            ]),
        ]))
        .block(block);
        frame.render_widget(welcome, area);
        return;
    }

    let chat_w = inner.width.saturating_sub(2) as usize;
    let max_bubble = (chat_w * 3 / 5).max(12);

    let mut lines: Vec<Line<'_>> = Vec::new();

    for msg in &app.messages {
        if msg.kind != MessageKind::Application {
            continue;
        }
        let is_me = msg.sender_inbox_id == app.inbox_id;
        let body = decode_body(msg);
        let time = format_relative(msg.sent_at_ns);

        let wrapped = wrap_text(&body, max_bubble.saturating_sub(4));
        let content_w = wrapped
            .iter()
            .map(|l| UnicodeWidthStr::width(l.as_str()))
            .max()
            .unwrap_or(0);
        let box_w = content_w + 2;
        let total_w = box_w + 2;

        if is_me {
            let status = delivery_icon(msg.delivery_status);
            let header = format!("{time}  {status}");
            let h_width = UnicodeWidthStr::width(header.as_str());
            let h_pad = chat_w.saturating_sub(h_width);
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(h_pad)),
                Span::styled(header, Style::default().fg(DIM)),
            ]));

            let b_pad = chat_w.saturating_sub(total_w);
            let top = format!("╭{}╮", "─".repeat(box_w));
            let bot = format!("╰{}╯", "─".repeat(box_w));
            let style = Style::default().fg(SELF_BG);

            lines.push(Line::from(vec![
                Span::raw(" ".repeat(b_pad)),
                Span::styled(top, style),
            ]));
            for wl in &wrapped {
                let pad = content_w.saturating_sub(UnicodeWidthStr::width(wl.as_str()));
                let row = format!("│ {}{} │", wl, " ".repeat(pad));
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(b_pad)),
                    Span::styled(row, style),
                ]));
            }
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(b_pad)),
                Span::styled(bot, style),
            ]));
        } else {
            let sender = truncate_id(&msg.sender_inbox_id, 12);
            lines.push(Line::from(vec![
                Span::styled(format!("  {sender}"), Style::default().fg(PEER_BG)),
                Span::styled(format!("  {time}"), Style::default().fg(DIM)),
            ]));

            let top = format!("  ╭{}╮", "─".repeat(box_w));
            let bot = format!("  ╰{}╯", "─".repeat(box_w));
            let style = Style::default().fg(PEER_BG);

            lines.push(Line::from(Span::styled(top, style)));
            for wl in &wrapped {
                let pad = content_w.saturating_sub(UnicodeWidthStr::width(wl.as_str()));
                let row = format!("  │ {}{} │", wl, " ".repeat(pad));
                lines.push(Line::from(Span::styled(row, style)));
            }
            lines.push(Line::from(Span::styled(bot, style)));
        }
        lines.push(Line::default());
    }

    // Scroll: offset 0 = pinned to bottom.
    let view_h = inner.height as usize;
    let total = lines.len();
    let max_offset = total.saturating_sub(view_h);
    let offset = app.scroll.min(max_offset);
    let start = total.saturating_sub(view_h + offset);
    let end = total.saturating_sub(offset);
    let visible: Vec<Line<'_>> = lines.into_iter().skip(start).take(end - start).collect();

    let paragraph = Paragraph::new(Text::from(visible))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);

    if offset > 0 {
        let indicator = format!(" ↑{offset} ");
        #[allow(clippy::cast_possible_truncation)]
        let x = area.x + area.width.saturating_sub(indicator.len() as u16 + 2);
        frame.render_widget(
            Paragraph::new(Span::styled(indicator, Style::default().fg(ACCENT))),
            Rect::new(x, area.y, 10, 1),
        );
    }
}

// ── Input bar ────────────────────────────────────────────────────

fn draw_input(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let is_overlay = matches!(app.mode, Mode::NewDm | Mode::NewGroup);
    let focused = (app.focus == Focus::Input && app.mode == Mode::Normal) || is_overlay;
    let border = if focused { ACCENT } else { DIM };

    let (title, prompt) = match app.mode {
        Mode::NewDm => (" New DM (wallet address) ", "0x> "),
        Mode::NewGroup => (" New Group (addresses, comma-sep) ", "0x> "),
        _ => ("", "> "),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));

    let display = format!("{prompt}{}", app.input);
    frame.render_widget(Paragraph::new(display).block(block), area);

    if focused {
        let before: String = app.input.chars().take(app.cursor).collect();
        let vis_offset = UnicodeWidthStr::width(before.as_str());
        #[allow(clippy::cast_possible_truncation)]
        let x = area.x + 1 + prompt.len() as u16 + vis_offset as u16;
        let y = area.y + 1;
        frame.set_cursor_position((x, y));
    }
}

// ── Status bar ───────────────────────────────────────────────────

fn draw_status(app: &App, frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Span::styled(&app.status, Style::default().fg(DIM))),
        area,
    );
}

// ── Help overlay ─────────────────────────────────────────────────

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    let w = 48.min(area.width.saturating_sub(4));
    let h = 18.min(area.height.saturating_sub(4));
    let popup = centered(area, w, h);

    let block = Block::default()
        .title(" Keyboard Shortcuts ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));

    let help = vec![
        Line::default(),
        help_line("1 / 2", "Switch Inbox / Requests tab"),
        help_line("j / k", "Navigate conversations"),
        help_line("Tab / Enter", "Open / focus input"),
        help_line("Esc", "Back to sidebar"),
        help_line("n", "New DM (wallet address)"),
        help_line("g", "New group chat"),
        help_line("m", "View group members"),
        help_line("a", "Accept request (Requests tab)"),
        help_line("x", "Reject request (Requests tab)"),
        help_line("r", "Sync conversations"),
        help_line("PgUp/Dn", "Scroll chat"),
        help_line("q", "Quit"),
        help_line("Ctrl-C", "Force quit"),
        Line::default(),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(DIM),
        )),
    ];

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(help).block(block), popup);
}

// ── Members overlay ──────────────────────────────────────────────

fn draw_members(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let w = 50.min(area.width.saturating_sub(4));
    #[allow(clippy::cast_possible_truncation)]
    let h = (app.members.len() as u16 + 4).min(area.height.saturating_sub(4));
    let popup = centered(area, w, h);

    let block = Block::default()
        .title(" Group Members ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));

    let mut lines = vec![Line::default()];
    for m in &app.members {
        let addr = truncate_id(&m.address, 32);
        lines.push(Line::from(vec![
            Span::styled(format!("  {addr}"), Style::default().fg(PEER_BG)),
            Span::styled(format!("  ({})", m.role), Style::default().fg(DIM)),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Esc to close",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn help_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {key:>12}  "), Style::default().fg(ACCENT)),
        Span::raw(desc),
    ])
}

const fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

// ── Helpers ──────────────────────────────────────────────────────

/// Simple word-wrap respecting unicode display width.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let max_w = max_width.max(8);
    let mut result = Vec::new();
    for raw in text.lines() {
        if raw.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut width = 0usize;
        for word in raw.split_whitespace() {
            let ww = UnicodeWidthStr::width(word);
            if width > 0 && width + 1 + ww > max_w {
                result.push(std::mem::take(&mut line));
                word.clone_into(&mut line);
                width = ww;
            } else {
                if width > 0 {
                    line.push(' ');
                    width += 1;
                }
                line.push_str(word);
                width += ww;
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Format a nanosecond timestamp as relative time.
#[allow(clippy::cast_possible_truncation)]
fn format_relative(ns: i64) -> String {
    let now_ns = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    let secs = (now_ns - ns) / 1_000_000_000;
    if secs < 60 {
        "now".into()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}
