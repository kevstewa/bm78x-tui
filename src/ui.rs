//! Drawing the screens.

use bm78x::protocol::{Reading, Readout};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Chart, Clear, Dataset, GraphType, List, ListItem, ListState, Paragraph,
};

use crate::app::{App, Link, Overlay};
use crate::bigtext;
use crate::format::{display_text, flag_names, in_display_units, unit_text};
use crate::trend::y_bounds;
use unicode_width::UnicodeWidthChar;

/// Draws the meter screen, and any popup over it.
pub fn draw(frame: &mut Frame, app: &App) {
    draw_meter(frame, app);
    match &app.overlay {
        Some(Overlay::Meters) => draw_menu(frame, app),
        Some(Overlay::LogDir(input)) => draw_log_dir(frame, input),
        None => {}
    }
}

/// A box of `width` by `height` in the middle of `area`, no bigger than it.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn draw_menu(frame: &mut Frame, app: &App) {
    let entries = app.menu_entries();
    let rows = u16::try_from(entries.len().max(3)).unwrap_or(u16::MAX);
    let area = centred(frame.area(), 64, rows.saturating_add(4));
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(" Meters ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, help] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let body = match &app.scan_error {
        Some(error) => {
            let [body, problem] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(body);
            frame.render_widget(
                Line::from(format!("Scanning failed: {error}")).red(),
                problem,
            );
            body
        }
        None => body,
    };
    if entries.is_empty() {
        let lines = vec![
            Line::from("Scanning for meters…"),
            Line::from(""),
            Line::from("Turn the meter's Bluetooth on: hold Δ for a second."),
        ];
        frame.render_widget(Paragraph::new(lines), body);
    } else {
        let items: Vec<ListItem> = entries
            .iter()
            .map(|e| {
                let name = e.name.as_deref().unwrap_or("?");
                let signal = match e.rssi {
                    Some(rssi) => format!("{rssi} dBm"),
                    None => "not seen yet".into(),
                };
                let line = Line::from(format!("{}  {name:<16}  {signal}", e.address));
                ListItem::new(if e.seen { line } else { line.dim() })
            })
            .collect();
        let list = List::new(items)
            .highlight_symbol("▶ ")
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let selected = app.selected();
        let index = entries.iter().position(|e| Some(e.address) == selected);
        let mut state = ListState::default().with_selected(index);
        frame.render_stateful_widget(list, body, &mut state);
    }
    frame.render_widget(
        Line::from("↑/↓ choose  Enter connect  x forget  s scan again  Esc close").dim(),
        help,
    );
}

fn draw_log_dir(frame: &mut Frame, input: &str) {
    let area = centred(frame.area(), 64, 4);
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(" Log directory ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [field, help] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    // Show the end of a long path, where the typing happens, in as many
    // columns as fit beside the cursor.
    let room = usize::from(field.width.saturating_sub(1));
    let mut width = 0;
    let start = input
        .char_indices()
        .rev()
        .find(|&(_, c)| {
            width += c.width().unwrap_or(0);
            width > room
        })
        .map_or(0, |(i, c)| i + c.len_utf8());
    let shown = &input[start..];
    frame.render_widget(Line::from(format!("{shown}█")), field);
    frame.render_widget(
        Line::from("Enter save  Ctrl-U clear  Esc cancel").dim(),
        help,
    );
}

fn draw_meter(frame: &mut Frame, app: &App) {
    let [header, display, stats, main, status, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(bigtext::HEIGHT as u16 + 2),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(header_line(app), header);
    draw_display(frame, app, display);
    frame.render_widget(stats_line(app), stats);
    if app.raw_view {
        draw_raw(frame, app, main);
    } else {
        draw_trend(frame, app, main);
    }
    frame.render_widget(status_line(app), status);
    frame.render_widget(
        Line::from("q quit  m meters  p pause  c clear  w/W window  l log  d log dir  r raw").dim(),
        help,
    );
}

fn header_line(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(reading) = &app.reading {
        spans.push(Span::from(reading.function.to_string()).bold());
        for name in flag_names(&reading.flags) {
            spans.push(Span::from(format!("  {name}")).yellow());
        }
    }
    if app.info.is_some_and(|i| i.battery_low()) {
        spans.push(Span::from("  BATTERY LOW").red().bold());
    }
    if let Some(address) = app.address {
        spans.push(Span::from(format!("   {address}")).dim());
    }
    Line::from(spans)
}

fn draw_display(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(reading) = &app.reading else {
        let text = match (app.address, &app.link) {
            (None, _) => "No meter. Press m to choose one.",
            (Some(_), Link::Connecting) => "Connecting…",
            _ => "Waiting for the meter…",
        };
        let paragraph = Paragraph::new(text).alignment(Alignment::Center).dim();
        frame.render_widget(paragraph, inner);
        return;
    };

    let text = display_text(reading);
    let unit = unit_text(reading);
    let style = display_style(app, reading);
    let unit_width = unit.chars().count() + 2;
    let lines: Vec<Line> = if bigtext::width(&text) + unit_width <= usize::from(inner.width) {
        let rows = bigtext::render(&text);
        let last = rows.len() - 1;
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| {
                // The unit sits on the bottom row; the other rows are padded
                // to the same width so centring keeps the digits aligned.
                let tail = if i == last {
                    format!("  {unit}")
                } else {
                    " ".repeat(unit_width)
                };
                Line::from(vec![
                    Span::styled(row, style),
                    Span::styled(tail, style.bold()),
                ])
            })
            .collect()
    } else {
        vec![Line::styled(format!("{text} {unit}"), style.bold())]
    };
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn display_style(app: &App, reading: &Reading) -> Style {
    if app.paused {
        Style::new().fg(Color::Yellow)
    } else if app.link != Link::Connected {
        Style::new().fg(Color::DarkGray)
    } else if reading.readout() == Readout::Overload {
        Style::new().fg(Color::Red)
    } else {
        Style::new().fg(Color::Cyan)
    }
}

fn stats_line(app: &App) -> Line<'static> {
    let Some(reading) = &app.reading else {
        return Line::from("");
    };
    let show = |value: Option<f64>| {
        value
            .and_then(|v| in_display_units(v, reading))
            .unwrap_or_else(|| "–".into())
    };
    Line::from(format!(
        "MIN {}   MAX {}   AVG {}   ({} readings)",
        show(app.stats.min()),
        show(app.stats.max()),
        show(app.stats.mean()),
        app.stats.count()
    ))
}

fn draw_trend(frame: &mut Frame, app: &App, area: Rect) {
    let span = f64::from(app.window_secs());
    let title = format!(" Trend, last {} ", window_label(app.window_secs()));
    let block = Block::bordered().title(title);

    // Plot in the units on the display, so the axis reads like the LCD.
    let reading = app.reading;
    let scale = reading.and_then(|r| r.prefix.multiplier()).unwrap_or(1.0);
    let points: Vec<(f64, f64)> = app
        .trend
        .window(app.plot_time(), span)
        .into_iter()
        .map(|(t, v)| (t, v / scale))
        .collect();
    let [low, high] = y_bounds(&points);
    let decimals = reading.map_or(2, |r| usize::from(r.decimals()));
    let unit = reading.as_ref().map(unit_text).unwrap_or_default();
    let y_label = |v: f64| format!("{v:.decimals$} {unit}");

    let dataset = Dataset::default()
        // Half blocks rather than Braille dots: every console font has
        // them, and they are solid enough to see.
        .marker(Marker::HalfBlock)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(Color::LightCyan))
        .data(&points);
    let x_labels = [
        format!("-{}", window_label(app.window_secs())),
        "0 s".into(),
    ];
    let chart = Chart::new(vec![dataset])
        .block(block)
        .x_axis(Axis::default().bounds([-span, 0.0]).labels(x_labels))
        .y_axis(Axis::default().bounds([low, high]).labels([
            y_label(low),
            y_label((low + high) / 2.0),
            y_label(high),
        ]));
    frame.render_widget(chart, area);
}

fn window_label(seconds: u32) -> String {
    if seconds <= 60 {
        format!("{seconds} s")
    } else {
        format!("{} min", seconds / 60)
    }
}

fn draw_raw(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" Raw frames, newest first ");
    let mut lines = Vec::new();
    for entry in app.raw.iter().rev() {
        let bytes: Vec<&str> = (0..entry.hex.len())
            .step_by(2)
            .map(|i| &entry.hex[i..i + 2])
            .collect();
        if bytes.len() == 152 {
            // The info packet and the one reading packet in use; the other
            // three are zeros.
            lines.push(Line::from(bytes[..24].join(" ")));
            lines.push(Line::from(bytes[24..56].join(" ")));
        } else {
            lines.push(Line::from(bytes.join(" ")).red());
        }
        lines.push(Line::from(format!("  {}", entry.decoded)).cyan());
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn status_line(app: &App) -> Line<'static> {
    let mut spans = vec![match &app.link {
        _ if app.address.is_none() => Span::from("Not connected"),
        Link::Connecting => Span::from("Connecting…").yellow(),
        Link::Connected => Span::from("Connected").green(),
        Link::Idle => Span::from("No data from the meter").yellow(),
        Link::Retrying(reason) => Span::from(format!("Reconnecting: {reason}")).yellow(),
        Link::Failed(reason) => Span::from(format!("Failed: {reason}")).red().bold(),
    }];
    if let Some(mtu) = app.mtu {
        spans.push(Span::from(format!("  MTU {mtu}")));
    }
    if app.lost > 0 {
        spans.push(Span::from(format!("  {} lost", app.lost)).red());
    }
    if app.bad_frames > 0 {
        spans.push(Span::from(format!("  {} bad", app.bad_frames)).red());
    }
    if app.paused {
        spans.push(Span::from("  PAUSED").yellow().bold());
    }
    if let Some(log) = &app.log {
        // The name alone: the directory would crowd out the rest.
        let name = log.path.file_name().unwrap_or(log.path.as_os_str());
        spans.push(
            Span::from(format!(
                "  LOG {} ({} rows)",
                name.to_string_lossy(),
                log.csv.rows()
            ))
            .magenta(),
        );
    }
    if let Some(message) = &app.message {
        spans.push(Span::from(format!("  {message}")).dim());
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{METER, char_key, connected, frame_event, volts, wall};
    use crate::app::{Link, Update};
    use bm78x::bluez::Discovered;
    use bm78x::protocol::MacAddress;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    const OTHER: MacAddress = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x66]);
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The screen as text, one string per row.
    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn contains(lines: &[String], text: &str) -> bool {
        lines.iter().any(|l| l.contains(text))
    }

    #[test]
    fn menu_while_scanning() {
        let (app, _) = App::new(None);
        let lines = screen(&app, 80, 24);
        assert!(contains(&lines, "Meters"), "{lines:#?}");
        assert!(contains(&lines, "Scanning"), "{lines:#?}");
        assert!(contains(&lines, "hold Δ"), "{lines:#?}");
        assert!(contains(&lines, "Not connected"), "{lines:#?}");
    }

    #[test]
    fn menu_lists_saved_and_found_meters() {
        let (mut app, _) = App::new(None);
        app.saved.remember(OTHER, Some("Bench meter".into()));
        let mut found = Discovered::new(METER);
        found.name = Some("BM78xBT".into());
        found.rssi = Some(-71);
        app.on_update(Update::Discovered(found), 0.0, wall());
        let lines = screen(&app, 80, 24);
        let row = |text| lines.iter().position(|l| l.contains(text));
        let saved = row("00:11:22:33:44:66").expect("saved meter listed");
        let found = row("00:11:22:33:44:55").expect("found meter listed");
        assert!(saved < found, "saved first: {lines:#?}");
        assert!(lines[saved].contains("Bench meter") && lines[saved].contains("not seen yet"));
        assert!(lines[found].contains("BM78xBT") && lines[found].contains("-71 dBm"));
    }

    #[test]
    fn a_failed_scan_is_shown_with_meters_listed() {
        let (mut app, _) = App::new(None);
        app.saved.remember(OTHER, Some("Bench meter".into()));
        app.on_update(Update::ScanFailed("adapter is off".into()), 0.0, wall());
        let lines = screen(&app, 80, 24);
        assert!(contains(&lines, "Bench meter"), "{lines:#?}");
        assert!(
            contains(&lines, "Scanning failed: adapter is off"),
            "{lines:#?}"
        );
        assert!(contains(&lines, "s scan again"), "{lines:#?}");
    }

    #[test]
    fn a_meter_bluez_remembers_but_has_not_heard_is_not_in_range() {
        let (mut app, _) = App::new(None);
        app.on_update(Update::Discovered(Discovered::new(METER)), 0.0, wall());
        let lines = screen(&app, 80, 24);
        let row = lines
            .iter()
            .find(|l| l.contains("00:11:22:33:44:55"))
            .unwrap();
        assert!(row.contains("not seen yet"), "{lines:#?}");
    }

    #[test]
    fn the_log_dir_dialog_fits_wide_characters() {
        // Each of these takes two columns; counting characters would push
        // the cursor off the end.
        let mut app = connected();
        app.log_dir = "/".into();
        app.on_key(char_key('d'));
        app.on_paste(&"測".repeat(60));
        let lines = screen(&app, 80, 24);
        // The test buffer shows a wide character's second column as a space.
        assert!(contains(&lines, "測 █"), "{lines:#?}");
    }

    #[test]
    fn menu_is_drawn_over_the_meter_screen() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        app.on_key(char_key('m'));
        let lines = screen(&app, 80, 24);
        assert!(contains(&lines, "Meters"), "{lines:#?}");
        // The meter screen is still there around the popup.
        assert!(contains(&lines, "DCV"), "{lines:#?}");
        assert!(contains(&lines, "Connected"), "{lines:#?}");
    }

    #[test]
    fn log_dir_dialog_shows_what_is_typed() {
        let mut app = connected();
        app.log_dir = "/tmp/logs".into();
        app.on_key(char_key('d'));
        app.on_key(char_key('x'));
        let lines = screen(&app, 80, 24);
        assert!(contains(&lines, "Log directory"), "{lines:#?}");
        assert!(contains(&lines, "/tmp/logsx█"), "{lines:#?}");
    }

    #[test]
    fn without_a_meter_the_screen_says_how_to_choose_one() {
        let (mut app, _) = App::new(None);
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let lines = screen(&app, 80, 24);
        assert!(contains(&lines, "Press m to choose one"), "{lines:#?}");
    }

    #[test]
    fn trend_line_is_solid() {
        // Braille dots were too faint to see on a Linux console.
        let mut app = connected();
        for n in 0..50 {
            app.on_update(
                frame_event(volts(32000 + n * 10)),
                f64::from(n) * 0.2,
                wall(),
            );
        }
        let lines = screen(&app, 80, 24);
        let below_display = &lines[8..];
        assert!(
            below_display.iter().any(|l| l.contains(['▀', '▄', '█'])),
            "{lines:#?}"
        );
        assert!(
            !below_display
                .iter()
                .any(|l| l.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)))
        );
    }

    #[test]
    fn meter_screen_shows_the_reading() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        app.on_update(frame_event(volts(32979)), 1.2, wall());
        let lines = screen(&app, 80, 24);
        for text in [
            "DCV",
            "AUTO",
            "V",
            "MIN 3.2979 V",
            "MAX 3.2981 V",
            "AVG 3.2980 V",
            "60 s",
            "Connected",
        ] {
            assert!(contains(&lines, text), "{text} missing from {lines:#?}");
        }
    }

    #[test]
    fn meter_screen_shows_pause_logging_and_problems() {
        let mut app = connected();
        app.on_update(frame_event(volts(1)), 1.0, wall());
        app.on_key(char_key('p'));
        app.lost = 4;
        app.link = Link::Retrying("the meter did not respond in time".into());
        let lines = screen(&app, 100, 24);
        for text in ["PAUSED", "4 lost", "did not respond"] {
            assert!(contains(&lines, text), "{text} missing from {lines:#?}");
        }
    }

    #[test]
    fn status_shows_the_log_file_not_its_directory() {
        let mut app = connected();
        let out = crate::app::tests::Shared::default();
        let path =
            std::path::PathBuf::from("/a/rather/long/log/directory/bm78x-20261023-145349.csv");
        app.start_logging(path, Box::new(out));
        app.message = None;
        let lines = screen(&app, 80, 24);
        assert!(
            contains(&lines, "LOG bm78x-20261023-145349.csv (0 rows)"),
            "{lines:#?}"
        );
        assert!(!contains(&lines, "/a/rather/long"), "{lines:#?}");
    }

    #[test]
    fn raw_view_shows_bytes() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        app.on_key(char_key('r'));
        let lines = screen(&app, 120, 24);
        assert!(contains(&lines, "ff 01 18 04"), "{lines:#?}");
        assert!(contains(&lines, "3.2981 V"), "{lines:#?}");
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        for (w, h) in [(1, 1), (10, 3), (30, 8)] {
            screen(&app, w, h);
            app.on_key(char_key('r'));
            screen(&app, w, h);
        }
        let (menu, _) = App::new(None);
        screen(&menu, 5, 2);
        let mut dialog = connected();
        dialog.on_key(char_key('d'));
        screen(&dialog, 5, 2);
    }
}
