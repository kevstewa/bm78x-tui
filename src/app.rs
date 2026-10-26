//! The application state: what the screens show and how keys and meter
//! events change it. No terminal or Bluetooth I/O happens here; `main` does
//! the [`Action`]s this returns, so all of it can be tested directly.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;

use bm78x::bluez::{BluezError, Discovered};
use bm78x::protocol::Frame;
use bm78x::protocol::{DeviceInfo, MacAddress, Reading};
use bm78x::session::{DisconnectReason, SessionError, SessionEvent};
use chrono::{DateTime, FixedOffset};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::csv::CsvLog;
use crate::format::flag_names;
use crate::saved::{Change, Saved};
use crate::stats::Stats;
use crate::trend::{Trend, WINDOWS};

/// How many frames the raw view keeps.
pub const RAW_KEEP: usize = 100;

/// The MTU Brymen requires for a frame to fit one notification.
pub const REQUIRED_MTU: u16 = 185;

/// A popup over the meter screen. While one is open it gets the keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// Choosing a meter: saved ones and those found by scanning.
    Meters,
    /// Editing the log directory; holds the text typed so far.
    LogDir(String),
}

/// One line of the meter menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// The meter's address.
    pub address: MacAddress,
    /// Its name, from the scan or as saved.
    pub name: Option<String>,
    /// Signal strength, if this scan has heard it.
    pub rssi: Option<i16>,
    /// Whether this scan has heard it.
    pub seen: bool,
    /// Whether it is saved from an earlier connection.
    pub saved: bool,
}

/// The state of the link to the meter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Connecting for the first time.
    Connecting,
    /// Connected and receiving.
    Connected,
    /// Connected, but the meter has gone quiet.
    Idle,
    /// Lost or failed; the session is retrying. Holds the reason.
    Retrying(String),
    /// Failed for good, such as a wrong password. Holds the reason.
    Failed(String),
}

/// News from the background tasks.
#[derive(Debug)]
pub enum Update {
    /// The scan found a meter, or its signal strength changed.
    Discovered(Discovered),
    /// Scanning failed.
    ScanFailed(String),
    /// A session reported something. Each session has a number, from
    /// [`App::session`] when it was started, so news from an old one can be
    /// told apart.
    Session {
        /// The session's number.
        session: u64,
        /// What it reported.
        event: SessionEvent,
    },
    /// The ATT MTU, after connecting.
    Mtu {
        /// The session's number.
        session: u64,
        /// The MTU, if BlueZ reports it.
        mtu: Option<u16>,
    },
    /// A session ended with an error retrying cannot fix.
    SessionFailed {
        /// The session's number.
        session: u64,
        /// What went wrong.
        error: String,
    },
}

/// Something for `main` to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Exit.
    Quit,
    /// Stop scanning and any session, and connect to this meter.
    Connect(MacAddress),
    /// Open a new CSV file and pass it to [`App::start_logging`].
    StartLogging,
    /// Start scanning, reporting meters with [`Update::Discovered`].
    StartScan,
    /// Stop scanning.
    StopScan,
    /// Check that this is a directory and pass it to [`App::set_log_dir`].
    SetLogDir(PathBuf),
}

/// One frame in the raw view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    /// The notification in hex.
    pub hex: String,
    /// What it decoded to, or why it did not.
    pub decoded: String,
}

/// A CSV log in progress.
pub struct Log {
    /// Where it is being written.
    pub path: PathBuf,
    /// The writer.
    pub csv: CsvLog<Box<dyn Write + Send>>,
}

/// The application state.
pub struct App {
    /// The open popup, if any.
    pub overlay: Option<Overlay>,
    /// Meters found by scanning, in the order first seen.
    pub meters: Vec<Discovered>,
    /// The meter highlighted in the menu.
    selected: Option<MacAddress>,
    /// What is remembered between runs.
    pub saved: Saved,
    /// Changes to `saved` that `main` has yet to write.
    pub saved_changes: Vec<Change>,
    /// The number of the session for [`App::address`]; see
    /// [`Update::Session`].
    pub session: u64,
    /// Whether to remember the meter when it connects: set when the user
    /// picks one, so a meter forgotten while connected stays forgotten when
    /// the session reconnects by itself.
    remember: bool,
    /// Where new logs go.
    pub log_dir: PathBuf,
    /// Why scanning failed, if it did.
    pub scan_error: Option<String>,
    /// The meter connected to, or being connected to.
    pub address: Option<MacAddress>,
    /// The state of the link.
    pub link: Link,
    /// The ATT MTU, once known.
    pub mtu: Option<u16>,
    /// The reading on display.
    pub reading: Option<Reading>,
    /// The latest device information packet.
    pub info: Option<DeviceInfo>,
    /// MIN, MAX and AVG since the last clear or function change.
    pub stats: Stats,
    /// The trend plot's history.
    pub trend: Trend,
    /// The plot window, as an index into [`WINDOWS`].
    pub window: usize,
    /// Whether the display, statistics and trend are frozen.
    pub paused: bool,
    /// When the pause began, for freezing the plot.
    paused_at: f64,
    /// Whether the raw view replaces the trend plot.
    pub raw_view: bool,
    /// The most recent frames, newest last.
    pub raw: VecDeque<RawEntry>,
    /// Notifications dropped by the transport.
    pub lost: u64,
    /// Notifications that did not decode.
    pub bad_frames: u64,
    /// The CSV log, if logging.
    pub log: Option<Log>,
    /// A message for the status bar.
    pub message: Option<String>,
    /// The current time in seconds, for the plot.
    pub now: f64,
}

impl App {
    /// Starts connecting to `address`, or with the meter menu open, and
    /// returns what to do first.
    pub fn new(address: Option<MacAddress>) -> (Self, Option<Action>) {
        let app = App {
            overlay: if address.is_some() {
                None
            } else {
                Some(Overlay::Meters)
            },
            meters: Vec::new(),
            selected: None,
            saved: Saved::default(),
            saved_changes: Vec::new(),
            session: 0,
            remember: address.is_some(),
            log_dir: PathBuf::from("."),
            scan_error: None,
            address,
            link: Link::Connecting,
            mtu: None,
            reading: None,
            info: None,
            stats: Stats::default(),
            trend: Trend::default(),
            // A minute.
            window: 2,
            paused: false,
            paused_at: 0.0,
            raw_view: false,
            raw: VecDeque::new(),
            lost: 0,
            bad_frames: 0,
            log: None,
            message: None,
            now: 0.0,
        };
        let first = match address {
            Some(address) => Action::Connect(address),
            None => Action::StartScan,
        };
        (app, Some(first))
    }

    /// Handles a key press.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        match self.overlay {
            Some(Overlay::Meters) => self.on_menu_key(key.code),
            Some(Overlay::LogDir(_)) => self.on_log_dir_key(key),
            None => self.on_meter_key(key.code),
        }
    }

    fn on_menu_key(&mut self, code: KeyCode) -> Option<Action> {
        let entries = self.menu_entries();
        let index = selected_index(&entries, self.selected);
        let at = |i: usize| entries.get(i).map(|e| e.address);
        match code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Esc | KeyCode::Char('m') => {
                self.overlay = None;
                return Some(Action::StopScan);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(i) = index {
                    self.selected = at(i.saturating_sub(1));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(i) = index {
                    self.selected = at(i + 1).or(at(i));
                }
            }
            KeyCode::Char('s') => return Some(self.start_scan()),
            KeyCode::Char('x') => {
                let i = index?;
                if entries[i].saved {
                    self.change(Change::Forget(entries[i].address));
                    // Highlight what is now in its place.
                    let left = self.menu_entries();
                    let i = i.min(left.len().saturating_sub(1));
                    self.selected = left.get(i).map(|e| e.address);
                }
            }
            KeyCode::Enter => {
                let address = at(index?)?;
                let working = !matches!(self.link, Link::Failed(_));
                if Some(address) == self.address && working {
                    self.overlay = None;
                    return Some(Action::StopScan);
                }
                self.switch_to(address);
                return Some(Action::Connect(address));
            }
            _ => {}
        }
        None
    }

    /// Opens the meter menu, highlighting the meter in use.
    fn open_menu(&mut self) -> Action {
        self.overlay = Some(Overlay::Meters);
        self.selected = self.address;
        let scan = self.start_scan();
        self.settle_selection();
        scan
    }

    /// Keeps the highlight on a meter that is in the menu, so lines added
    /// above it do not move it to another.
    fn settle_selection(&mut self) {
        let entries = self.menu_entries();
        if !entries.iter().any(|e| Some(e.address) == self.selected) {
            self.selected = entries.first().map(|e| e.address);
        }
    }

    /// Forgets what the last scan found, and scans again.
    fn start_scan(&mut self) -> Action {
        self.meters.clear();
        self.scan_error = None;
        Action::StartScan
    }

    /// Changes what is remembered, now and, through `main`, on disk.
    fn change(&mut self, change: Change) {
        self.saved.apply(&change);
        self.saved_changes.push(change);
    }

    /// The meter highlighted in the menu.
    pub fn selected(&self) -> Option<MacAddress> {
        let entries = self.menu_entries();
        selected_index(&entries, self.selected).map(|i| entries[i].address)
    }

    /// Handles text pasted into the terminal: typed into the log directory
    /// dialog, if it is open, without line breaks or other control
    /// characters.
    pub fn on_paste(&mut self, text: &str) {
        if let Some(Overlay::LogDir(input)) = &mut self.overlay {
            input.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    fn on_log_dir_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(Overlay::LogDir(input)) = &mut self.overlay else {
            return None;
        };
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => input.clear(),
            // Shift makes capitals; Ctrl and Alt make editing keys, which
            // this dialog does not have.
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                input.push(c)
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Esc => self.overlay = None,
            KeyCode::Enter => {
                let dir = PathBuf::from(std::mem::take(input));
                self.overlay = None;
                return Some(Action::SetLogDir(dir));
            }
            _ => {}
        }
        None
    }

    /// Forgets the current meter's readings and starts on `address`.
    fn switch_to(&mut self, address: MacAddress) {
        if let Some(log) = self.log.take() {
            self.message = Some(format!(
                "Saved {} rows to {}",
                log.csv.rows(),
                log.path.display()
            ));
        }
        self.overlay = None;
        self.address = Some(address);
        self.session += 1;
        self.remember = true;
        self.link = Link::Connecting;
        self.mtu = None;
        self.reading = None;
        self.info = None;
        self.stats.clear();
        self.trend.clear();
        self.paused = false;
        self.raw.clear();
        self.lost = 0;
        self.bad_frames = 0;
    }

    /// The meter menu's lines: saved meters first, most recent first, then
    /// the others found, in the order found.
    pub fn menu_entries(&self) -> Vec<MenuEntry> {
        let found = |address| self.meters.iter().find(|m| m.address == address);
        let saved = self.saved.meters.iter().map(|m| {
            let found = found(m.address);
            let rssi = found.and_then(|f| f.rssi);
            MenuEntry {
                address: m.address,
                name: found
                    .and_then(|f| f.name.clone())
                    .or_else(|| m.name.clone()),
                rssi,
                seen: rssi.is_some(),
                saved: true,
            }
        });
        let others = self
            .meters
            .iter()
            .filter(|f| !self.saved.meters.iter().any(|m| m.address == f.address))
            .map(|f| MenuEntry {
                address: f.address,
                name: f.name.clone(),
                rssi: f.rssi,
                seen: f.rssi.is_some(),
                saved: false,
            });
        saved.chain(others).collect()
    }

    /// The time the plot ends at: now, or when the pause began.
    pub fn plot_time(&self) -> f64 {
        if self.paused {
            self.paused_at
        } else {
            self.now
        }
    }

    /// Reopens the log directory dialog with `typed`, saying why `main`
    /// refused it.
    pub fn reject_log_dir(&mut self, typed: String, reason: &str) {
        self.overlay = Some(Overlay::LogDir(typed));
        self.message = Some(format!("Log directory unchanged: {reason}"));
    }

    /// Sets the log directory, checked by `main`, and remembers it.
    pub fn set_log_dir(&mut self, dir: PathBuf) {
        self.message = Some(format!("New logs go to {}", dir.display()));
        self.change(Change::LogDir(dir.clone()));
        self.log_dir = dir;
    }

    fn on_meter_key(&mut self, code: KeyCode) -> Option<Action> {
        match code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('m') => return Some(self.open_menu()),
            KeyCode::Char('d') => {
                let current = self.log_dir.to_string_lossy().into_owned();
                self.overlay = Some(Overlay::LogDir(current));
            }
            KeyCode::Char('p') => {
                self.paused = !self.paused;
                self.paused_at = self.now;
            }
            KeyCode::Char('c') => {
                self.stats.clear();
                self.trend.clear();
            }
            KeyCode::Char('w') => self.window = (self.window + 1) % WINDOWS.len(),
            KeyCode::Char('W') => self.window = (self.window + WINDOWS.len() - 1) % WINDOWS.len(),
            KeyCode::Char('r') => self.raw_view = !self.raw_view,
            KeyCode::Char('l') => match self.log.take() {
                Some(log) => {
                    self.message = Some(format!(
                        "Saved {} rows to {}",
                        log.csv.rows(),
                        log.path.display()
                    ));
                }
                None if self.address.is_none() => {
                    self.message = Some("Choose a meter to log first: press m".into());
                }
                None => return Some(Action::StartLogging),
            },
            _ => {}
        }
        None
    }

    /// Handles news from a background task, received at `now` seconds and
    /// wall-clock time `wall`.
    pub fn on_update(&mut self, update: Update, now: f64, wall: DateTime<FixedOffset>) {
        self.now = now;
        let from = match &update {
            Update::Session { session, .. }
            | Update::Mtu { session, .. }
            | Update::SessionFailed { session, .. } => Some(*session),
            _ => None,
        };
        if from.is_some_and(|session| session != self.session) {
            // From a session already stopped, about a meter no longer shown.
            return;
        }
        match update {
            Update::Discovered(found) => {
                match self.meters.iter_mut().find(|m| m.address == found.address) {
                    Some(known) => *known = found,
                    None => self.meters.push(found),
                }
                self.settle_selection();
            }
            Update::ScanFailed(error) => self.scan_error = Some(error),
            Update::Mtu { mtu, .. } => {
                self.mtu = mtu;
                if let Some(mtu) = mtu.filter(|&m| m < REQUIRED_MTU) {
                    self.message = Some(format!(
                        "MTU {mtu} is below the {REQUIRED_MTU} the meter needs; frames may be cut short"
                    ));
                }
            }
            Update::SessionFailed { error, .. } => self.link = Link::Failed(error),
            Update::Session { event, .. } => self.on_session(event, now, wall),
        }
    }

    fn on_session(&mut self, event: SessionEvent, now: f64, wall: DateTime<FixedOffset>) {
        match event {
            SessionEvent::Connected { address, .. } => {
                self.address = Some(address);
                self.link = Link::Connected;
                let name = self
                    .meters
                    .iter()
                    .find(|m| m.address == address)
                    .and_then(|m| m.name.clone());
                if std::mem::take(&mut self.remember) {
                    self.settle_selection();
                    self.change(Change::Remember(address, name));
                }
            }
            SessionEvent::ConnectFailed(error) => {
                self.link = Link::Retrying(connect_failure(&error))
            }
            SessionEvent::ClockRefused(failure) => {
                self.message = Some(format!("The meter's clock was not set: {failure}"));
            }
            SessionEvent::Frame { frame, raw, .. } => {
                self.link = Link::Connected;
                self.keep_raw(&raw, describe(&frame));
                self.on_frame(&frame, now, wall);
            }
            SessionEvent::BadFrame { error, raw, .. } => {
                self.bad_frames += 1;
                self.keep_raw(&raw, format!("bad frame: {error}"));
            }
            SessionEvent::Idle => self.link = Link::Idle,
            SessionEvent::FramesLost(count) => self.lost += count,
            SessionEvent::Disconnected(reason) => {
                self.link = Link::Retrying(
                    match reason {
                        DisconnectReason::Stalled => "no data for too long",
                        _ => "link lost",
                    }
                    .into(),
                );
            }
            _ => {}
        }
    }

    fn on_frame(&mut self, frame: &Frame, now: f64, wall: DateTime<FixedOffset>) {
        self.info = Some(frame.info);
        let Some(reading) = frame.reading() else {
            return;
        };
        // Logging records every frame, paused or not.
        if let Some(log) = &mut self.log
            && let Err(error) = log.csv.write(wall, reading)
        {
            self.message = Some(format!("Logging stopped: {error}"));
            self.log = None;
        }
        if self.paused {
            return;
        }
        // A new function, unit or REL starts the statistics and trend
        // afresh; a new prefix from autoranging does not, since they are
        // kept in base units.
        let meaning = |r: &Reading| (r.function, r.unit, r.flags.relative());
        if let Some(previous) = &self.reading
            && meaning(previous) != meaning(reading)
        {
            self.stats.clear();
            self.trend.clear();
        }
        self.reading = Some(*reading);
        if is_live(reading)
            && let Some(value) = reading.si_value()
        {
            self.stats.push(value);
            self.trend.push(now, value);
        }
    }

    fn keep_raw(&mut self, raw: &[u8], decoded: String) {
        if self.raw.len() == RAW_KEEP {
            self.raw.pop_front();
        }
        let hex = raw.iter().map(|b| format!("{b:02x}")).collect();
        self.raw.push_back(RawEntry { hex, decoded });
    }

    /// Moves the clock on, so the plot scrolls while no frames arrive.
    pub fn tick(&mut self, now: f64) {
        self.now = now;
    }

    /// Starts logging to `out`, a new file at `path`.
    pub fn start_logging(&mut self, path: PathBuf, out: Box<dyn Write + Send>) {
        match CsvLog::new(out) {
            Ok(csv) => {
                self.message = Some(format!("Logging to {}", path.display()));
                self.log = Some(Log { path, csv });
            }
            Err(error) => {
                self.message = Some(format!("Cannot log to {}: {error}", path.display()));
            }
        }
    }

    /// The plot window in seconds.
    pub fn window_secs(&self) -> u32 {
        WINDOWS[self.window]
    }
}

/// The index of the menu line for `selected`, or of the first line if it is
/// not there; `None` for an empty menu.
fn selected_index(entries: &[MenuEntry], selected: Option<MacAddress>) -> Option<usize> {
    if entries.is_empty() {
        return None;
    }
    Some(
        entries
            .iter()
            .position(|e| Some(e.address) == selected)
            .unwrap_or(0),
    )
}

/// Whether the display shows a measurement being made now, rather than a
/// held one (HOLD, A-HOLD) or the meter's record of past ones (CREST, MAX,
/// MIN, AVG). Only those go into the statistics and trend.
fn is_live(reading: &Reading) -> bool {
    let f = reading.flags;
    !(f.hold() || f.auto_hold() || f.crest() || f.max() || f.min() || f.avg())
}

/// Why a connect failed, with what to do when the meter was not found.
fn connect_failure(error: &SessionError) -> String {
    let not_found = matches!(
        error,
        SessionError::Transport(e)
            if matches!(e.downcast_ref(), Some(BluezError::NotFound(_)))
    );
    if not_found {
        format!("{error}; is its Bluetooth on (hold Δ for a second)?")
    } else {
        error.to_string()
    }
}

/// One line about a frame for the raw view.
fn describe(frame: &Frame) -> String {
    match frame.reading() {
        Some(r) => {
            let flags = flag_names(&r.flags).join(" ");
            format!("{} {r} {flags}  clock {}", r.function, r.time)
        }
        None => "no reading".into(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::tests::reading;
    use crate::saved::Change;
    use bm78x::bluez::BluezError;
    use bm78x::protocol::{Category, Failure, Frame, Function, Prefix, Unit};
    use bm78x::session::{DisconnectReason, SessionError};
    use bm78x::transport::TransportError;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::{Arc, Mutex};

    pub const METER: MacAddress = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    const OTHER: MacAddress = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x66]);

    pub fn wall() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-10-23T14:05:01.234+02:00").unwrap()
    }

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub fn char_key(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    /// A frame event carrying `reading`.
    pub fn frame_event(reading: Reading) -> Update {
        let frame = Frame {
            info: DeviceInfo {
                category: Category::Multimeter,
                mac: METER,
                battery: 0,
                power_source: 0,
                reading_counts: [4, 0, 0],
                reading_number: 1,
            },
            readings: [Some(reading), None, None, None],
        };
        let raw = frame.encode().to_vec();
        news(SessionEvent::frame(frame, raw))
    }

    /// News from the first session.
    pub fn news(event: SessionEvent) -> Update {
        Update::Session { session: 0, event }
    }

    pub fn volts(raw: i32) -> Reading {
        reading(raw, 1, Prefix::Base, Unit::Volt, (0x10, 0))
    }

    /// An app connected to `METER`.
    pub fn connected() -> App {
        let (mut app, _) = App::new(Some(METER));
        app.on_update(news(SessionEvent::connected(METER)), 0.0, wall());
        app
    }

    pub fn discovered(address: MacAddress, rssi: Option<i16>) -> Update {
        let mut found = Discovered::new(address);
        found.name = Some("BM78xBT".into());
        found.rssi = rssi;
        Update::Discovered(found)
    }

    /// An app whose menu lists `saved` from earlier runs, then what the scan
    /// found.
    fn menu(saved: &[(MacAddress, &str)], found: &[(MacAddress, i16)]) -> App {
        let (mut app, _) = App::new(None);
        for &(address, name) in saved.iter().rev() {
            app.saved.remember(address, Some(name.into()));
        }
        for &(address, rssi) in found {
            app.on_update(discovered(address, Some(rssi)), 0.0, wall());
        }
        app
    }

    #[test]
    fn starts_with_the_meter_menu_open_without_an_address() {
        let (app, action) = App::new(None);
        assert_eq!(app.overlay, Some(Overlay::Meters));
        assert_eq!(action, Some(Action::StartScan));
    }

    #[test]
    fn connects_straight_away_with_an_address() {
        let (app, action) = App::new(Some(METER));
        assert_eq!(app.overlay, None);
        assert_eq!(app.link, Link::Connecting);
        assert_eq!(action, Some(Action::Connect(METER)));
    }

    #[test]
    fn scan_lists_each_meter_once() {
        let app = menu(&[], &[(METER, -70), (OTHER, -80), (METER, -60)]);
        let listed: Vec<_> = app.meters.iter().map(|m| (m.address, m.rssi)).collect();
        assert_eq!(listed, vec![(METER, Some(-60)), (OTHER, Some(-80))]);
    }

    #[test]
    fn menu_lists_saved_meters_first() {
        let mut app = menu(&[(METER, "Bench")], &[(OTHER, -80)]);
        let entry = |address, name: &str, rssi, seen, saved| MenuEntry {
            address,
            name: Some(name.into()),
            rssi,
            seen,
            saved,
        };
        assert_eq!(
            app.menu_entries(),
            vec![
                entry(METER, "Bench", None, false, true),
                entry(OTHER, "BM78xBT", Some(-80), true, false),
            ]
        );
        // Once the scan sees the saved meter, it stays where it is.
        app.on_update(discovered(METER, Some(-60)), 0.0, wall());
        assert_eq!(
            app.menu_entries()[0],
            entry(METER, "BM78xBT", Some(-60), true, true)
        );
        assert_eq!(app.menu_entries().len(), 2);
    }

    #[test]
    fn menu_selects_and_connects() {
        let mut app = menu(&[], &[]);
        assert_eq!(app.on_key(key(KeyCode::Enter)), None, "nothing to pick yet");
        app.on_update(discovered(METER, Some(-70)), 0.0, wall());
        app.on_update(discovered(OTHER, Some(-80)), 0.0, wall());
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected(), Some(OTHER), "stops at the last meter");
        app.on_key(key(KeyCode::Up));
        app.on_key(char_key('j'));
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::Connect(OTHER))
        );
        assert_eq!(app.overlay, None);
        assert_eq!(app.address, Some(OTHER));
        assert_eq!(app.link, Link::Connecting);
    }

    #[test]
    fn a_saved_meter_can_be_chosen_before_the_scan_sees_it() {
        let mut app = menu(&[(METER, "Bench")], &[]);
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::Connect(METER))
        );
    }

    #[test]
    fn m_opens_the_menu_and_esc_closes_it() {
        let mut app = connected();
        assert_eq!(app.on_key(char_key('m')), Some(Action::StartScan));
        assert_eq!(app.overlay, Some(Overlay::Meters));
        assert_eq!(app.on_key(key(KeyCode::Esc)), Some(Action::StopScan));
        assert_eq!(app.overlay, None);
        assert_eq!(app.address, Some(METER), "still on the same meter");
    }

    #[test]
    fn keys_go_to_the_open_menu() {
        let mut app = connected();
        app.on_key(char_key('m'));
        app.on_key(char_key('p'));
        app.on_key(char_key('r'));
        assert!(!app.paused && !app.raw_view);
    }

    #[test]
    fn switching_meters_starts_afresh() {
        let mut app = connected();
        app.on_update(frame_event(volts(1000)), 1.0, wall());
        app.on_key(char_key('l'));
        app.start_logging(PathBuf::from("bm78x-test.csv"), Box::new(Shared::default()));
        app.on_key(char_key('p'));

        app.on_key(char_key('m'));
        app.on_update(discovered(OTHER, Some(-80)), 2.0, wall());
        // The current meter is saved, so it is first; OTHER is next.
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::Connect(OTHER))
        );
        assert_eq!(app.reading, None);
        assert_eq!(app.info, None);
        assert_eq!(app.stats.count(), 0);
        assert!(app.trend.window(2.0, 600.0).is_empty());
        assert!(app.raw.is_empty());
        assert!(!app.paused);
        assert!(app.log.is_none(), "the log was for the other meter");
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("bm78x-test.csv")
        );
    }

    #[test]
    fn connecting_remembers_the_meter_and_its_name() {
        let mut app = menu(&[], &[(METER, -70)]);
        app.on_key(key(KeyCode::Enter));
        assert!(app.saved_changes.is_empty(), "not until it connects");
        let session = app.session;
        app.on_update(
            Update::Session {
                session,
                event: SessionEvent::connected(METER),
            },
            1.0,
            wall(),
        );
        assert_eq!(
            app.saved_changes,
            [Change::Remember(METER, Some("BM78xBT".into()))]
        );
        assert_eq!(app.saved.meters[0].address, METER);
        assert_eq!(app.saved.meters[0].name.as_deref(), Some("BM78xBT"));
    }

    #[test]
    fn news_from_an_old_session_is_ignored() {
        // Frames the old meter sent before its session stopped can still be
        // queued after a switch.
        let mut app = connected();
        app.on_key(char_key('m'));
        app.on_update(discovered(OTHER, Some(-80)), 1.0, wall());
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::Connect(OTHER))
        );
        app.saved_changes.clear();
        app.on_update(frame_event(volts(1000)), 2.0, wall());
        app.on_update(news(SessionEvent::connected(METER)), 2.0, wall());
        app.on_update(
            Update::Mtu {
                session: 0,
                mtu: Some(23),
            },
            2.0,
            wall(),
        );
        assert_eq!(app.reading, None);
        assert_eq!(app.address, Some(OTHER));
        assert_eq!(app.link, Link::Connecting);
        assert_eq!(app.mtu, None);
        assert!(app.saved_changes.is_empty());

        let session = app.session;
        assert_ne!(session, 0);
        app.on_update(
            Update::Session {
                session,
                event: SessionEvent::connected(OTHER),
            },
            3.0,
            wall(),
        );
        assert_eq!(app.link, Link::Connected);
    }

    #[test]
    fn the_highlight_stays_on_its_meter() {
        let mut app = menu(&[(METER, "Bench")], &[(OTHER, -80)]);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected(), Some(OTHER));
        // Connecting puts OTHER first in the saved list, above METER.
        app.on_key(key(KeyCode::Enter));
        app.on_key(char_key('m'));
        app.on_update(discovered(METER, Some(-60)), 1.0, wall());
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.selected(), Some(METER));
        let session = app.session;
        app.on_update(
            Update::Session {
                session,
                event: SessionEvent::connected(OTHER),
            },
            2.0,
            wall(),
        );
        assert_eq!(app.menu_entries()[0].address, OTHER);
        assert_eq!(app.selected(), Some(METER));
    }

    #[test]
    fn enter_on_the_connected_meter_just_closes_the_menu() {
        let mut app = connected();
        app.on_update(frame_event(volts(1000)), 1.0, wall());
        app.on_key(char_key('m'));
        assert_eq!(app.selected(), Some(METER));
        assert_eq!(app.on_key(key(KeyCode::Enter)), Some(Action::StopScan));
        assert_eq!(app.overlay, None);
        assert!(app.reading.is_some(), "the link and readings are kept");
        // After a failure, Enter tries again.
        app.on_update(
            Update::SessionFailed {
                session: 0,
                error: "the meter rejected the password".into(),
            },
            2.0,
            wall(),
        );
        app.on_key(char_key('m'));
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::Connect(METER))
        );
    }

    #[test]
    fn a_forgotten_meter_stays_forgotten_when_it_reconnects() {
        let mut app = connected();
        app.on_key(char_key('m'));
        app.on_key(char_key('x'));
        assert!(app.saved.meters.is_empty());
        // The link drops and the session reconnects by itself.
        app.on_update(
            news(SessionEvent::Disconnected(DisconnectReason::LinkLost)),
            1.0,
            wall(),
        );
        app.on_update(news(SessionEvent::connected(METER)), 2.0, wall());
        assert!(app.saved.meters.is_empty());
    }

    #[test]
    fn each_scan_starts_with_no_signals() {
        let mut app = connected();
        app.on_key(char_key('m'));
        app.on_update(discovered(OTHER, Some(-80)), 1.0, wall());
        app.on_key(key(KeyCode::Esc));
        // Reopened later, when OTHER may have been switched off.
        assert_eq!(app.on_key(char_key('m')), Some(Action::StartScan));
        let other = app.menu_entries().into_iter().find(|e| e.address == OTHER);
        assert_eq!(other, None);
        // BlueZ also reports meters it remembers but has not heard lately,
        // without a signal strength.
        app.on_update(discovered(OTHER, None), 2.0, wall());
        let other = app.menu_entries().into_iter().find(|e| e.address == OTHER);
        assert!(!other.unwrap().seen);
    }

    #[test]
    fn s_scans_again_after_a_failure() {
        let (mut app, _) = App::new(None);
        app.on_update(Update::ScanFailed("adapter off".into()), 1.0, wall());
        assert_eq!(app.on_key(char_key('s')), Some(Action::StartScan));
        assert_eq!(app.scan_error, None);
    }

    #[test]
    fn x_forgets_a_saved_meter() {
        let mut app = menu(&[(METER, "Bench")], &[(OTHER, -80)]);
        app.on_key(key(KeyCode::Down));
        app.on_key(char_key('x'));
        assert!(app.saved_changes.is_empty(), "OTHER is not saved");
        app.on_key(key(KeyCode::Up));
        app.on_key(char_key('x'));
        assert_eq!(app.saved_changes, [Change::Forget(METER)]);
        assert!(app.saved.meters.is_empty());
        // Not seen by this scan, so it leaves the menu.
        assert_eq!(app.menu_entries().len(), 1);
        assert_eq!(app.selected(), Some(OTHER));
    }

    #[test]
    fn log_dir_is_edited_in_a_dialog() {
        let mut app = connected();
        app.log_dir = PathBuf::from("/tmp/logs");
        assert_eq!(app.on_key(char_key('d')), None);
        assert_eq!(app.overlay, Some(Overlay::LogDir("/tmp/logs".into())));
        for _ in 0.."logs".len() {
            app.on_key(key(KeyCode::Backspace));
        }
        // Letters that are keys elsewhere are typed, not acted on.
        for c in "pq m".chars() {
            assert_eq!(app.on_key(char_key(c)), None);
        }
        assert!(!app.paused);
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::SetLogDir(PathBuf::from("/tmp/pq m")))
        );
        assert_eq!(app.overlay, None);

        app.on_key(char_key('d'));
        app.on_key(char_key('x'));
        assert_eq!(app.on_key(key(KeyCode::Esc)), None);
        assert_eq!(app.overlay, None);
        assert_eq!(app.log_dir, PathBuf::from("/tmp/logs"), "cancelled");
    }

    #[test]
    fn the_log_dir_dialog_types_only_plain_characters() {
        let mut app = connected();
        app.log_dir = PathBuf::from("/tmp");
        app.on_key(char_key('d'));
        // Ctrl-W and Alt-B are editing keys elsewhere, not letters.
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
        app.on_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
        assert_eq!(app.overlay, Some(Overlay::LogDir("/tmpL".into())));
    }

    #[test]
    fn pasting_into_the_log_dir_dialog() {
        let mut app = connected();
        app.log_dir = PathBuf::from("/");
        app.on_key(char_key('d'));
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        // A pasted line break is not Enter.
        app.on_paste("/home/me/logs\n");
        assert_eq!(app.overlay, Some(Overlay::LogDir("/home/me/logs".into())));
        // Pasting elsewhere does nothing.
        app.on_key(key(KeyCode::Esc));
        app.on_paste("q");
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn ctrl_u_clears_the_log_dir_field() {
        let mut app = connected();
        app.log_dir = PathBuf::from("/a/long/directory/name");
        app.on_key(char_key('d'));
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.overlay, Some(Overlay::LogDir(String::new())));
    }

    #[test]
    fn a_rejected_log_dir_reopens_the_dialog() {
        let mut app = connected();
        app.saved_changes.clear();
        app.reject_log_dir("/nope".into(), "it does not exist");
        assert_eq!(app.overlay, Some(Overlay::LogDir("/nope".into())));
        let message = app.message.clone().unwrap();
        assert!(
            message.starts_with("Log directory unchanged: it does not exist"),
            "{message}"
        );
        assert!(app.saved_changes.is_empty());
    }

    #[test]
    fn setting_the_log_dir_remembers_it() {
        let mut app = connected();
        app.set_log_dir(PathBuf::from("/tmp/logs"));
        assert_eq!(app.log_dir, PathBuf::from("/tmp/logs"));
        assert_eq!(app.saved.log_dir, Some(PathBuf::from("/tmp/logs")));
        assert_eq!(
            app.saved_changes.last(),
            Some(&Change::LogDir(PathBuf::from("/tmp/logs")))
        );
        assert!(app.message.as_deref().unwrap_or("").contains("/tmp/logs"));
    }

    #[test]
    fn quit_keys() {
        let (mut app, _) = App::new(None);
        assert_eq!(
            app.on_key(char_key('q')),
            Some(Action::Quit),
            "from the menu"
        );
        let mut meter = connected();
        assert_eq!(meter.on_key(char_key('q')), Some(Action::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        meter.on_key(char_key('d'));
        assert_eq!(meter.on_key(ctrl_c), Some(Action::Quit), "even in a dialog");
    }

    #[test]
    fn pause_freezes_the_plot() {
        let mut app = connected();
        app.on_update(frame_event(volts(1000)), 1.0, wall());
        app.tick(2.0);
        app.on_key(char_key('p'));
        let frozen = app.trend.window(app.plot_time(), 10.0);
        assert_eq!(frozen, vec![(-1.0, 1000.0 * 1e-4)]);
        app.tick(30.0);
        app.on_update(frame_event(volts(2000)), 31.0, wall());
        assert_eq!(app.trend.window(app.plot_time(), 10.0), frozen);
        app.on_key(char_key('p'));
        assert_eq!(app.plot_time(), 31.0);
    }

    #[test]
    fn frames_update_the_display_statistics_and_trend() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        app.on_update(frame_event(volts(32979)), 1.2, wall());
        assert_eq!(app.reading.unwrap().raw_value, 32979);
        assert_eq!(app.info.unwrap().mac, METER);
        assert_eq!(app.stats.count(), 2);
        assert_eq!(app.stats.max(), Some(3.2981));
        assert_eq!(app.trend.window(1.2, 10.0).len(), 2);
        assert_eq!(app.now, 1.2);
        assert_eq!(app.raw.len(), 2);
        assert!(app.raw[1].hex.starts_with("ff0118"), "{}", app.raw[1].hex);
        assert!(
            app.raw[1].decoded.contains("3.2979 V"),
            "{}",
            app.raw[1].decoded
        );
    }

    #[test]
    fn non_numeric_readings_do_not_enter_the_statistics() {
        let mut app = connected();
        app.on_update(
            frame_event(reading(0, 1, Prefix::Base, Unit::Volt, (0x10, 0x20))),
            1.0,
            wall(),
        );
        assert_eq!(app.stats.count(), 0);
        assert!(app.reading.is_some(), "OL is still displayed");
    }

    #[test]
    fn a_new_function_restarts_the_statistics() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        let mut ohms = reading(1000, 1, Prefix::Kilo, Unit::Ohm, (0x10, 0));
        ohms.function = Function::Resistance;
        app.on_update(frame_event(ohms), 2.0, wall());
        assert_eq!(app.stats.count(), 1);
        assert_eq!(app.trend.window(2.0, 10.0).len(), 1);
    }

    #[test]
    fn autoranging_keeps_the_statistics() {
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        let millivolts = reading(12345, 2, Prefix::Milli, Unit::Volt, (0x10, 0));
        app.on_update(frame_event(millivolts), 2.0, wall());
        assert_eq!(app.stats.count(), 2);
        assert_eq!(app.stats.min(), Some(12.345 * 1e-3));
    }

    #[test]
    fn pause_freezes_the_display_but_not_the_raw_view() {
        let mut app = connected();
        app.on_update(frame_event(volts(1000)), 1.0, wall());
        app.on_key(char_key('p'));
        assert!(app.paused);
        app.on_update(frame_event(volts(2000)), 2.0, wall());
        assert_eq!(app.reading.unwrap().raw_value, 1000);
        assert_eq!(app.stats.count(), 1);
        assert_eq!(app.trend.window(2.0, 10.0).len(), 1);
        assert_eq!(app.raw.len(), 2);
        app.on_key(char_key('p'));
        app.on_update(frame_event(volts(3000)), 3.0, wall());
        assert_eq!(app.reading.unwrap().raw_value, 3000);
    }

    #[test]
    fn clear_restarts_the_statistics_and_trend() {
        let mut app = connected();
        app.on_update(frame_event(volts(1000)), 1.0, wall());
        app.on_key(char_key('c'));
        assert_eq!(app.stats.count(), 0);
        assert!(app.trend.window(1.0, 10.0).is_empty());
        assert!(app.reading.is_some(), "the display stays");
    }

    #[test]
    fn window_keys_cycle() {
        let mut app = connected();
        assert_eq!(app.window_secs(), 60, "starts at a minute");
        app.on_key(char_key('w'));
        assert_eq!(app.window_secs(), 300);
        app.on_key(char_key('w'));
        app.on_key(char_key('w'));
        assert_eq!(app.window_secs(), 10, "wraps around");
        app.on_key(char_key('W'));
        assert_eq!(app.window_secs(), 600);
    }

    #[test]
    fn raw_view_toggles() {
        let mut app = connected();
        app.on_key(char_key('r'));
        assert!(app.raw_view);
        app.on_key(char_key('r'));
        assert!(!app.raw_view);
    }

    #[test]
    fn raw_view_keeps_the_newest_frames() {
        let mut app = connected();
        for n in 0..(RAW_KEEP as i32 + 5) {
            app.on_update(frame_event(volts(n)), f64::from(n), wall());
        }
        assert_eq!(app.raw.len(), RAW_KEEP);
        assert!(
            app.raw
                .back()
                .unwrap()
                .decoded
                .contains(&format!("{}", volts(RAW_KEEP as i32 + 4)))
        );
    }

    /// A writer whose bytes the test can read afterwards.
    #[derive(Clone, Default)]
    pub struct Shared(pub Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn logging_starts_records_and_stops() {
        let mut app = connected();
        assert_eq!(app.on_key(char_key('l')), Some(Action::StartLogging));
        let out = Shared::default();
        app.start_logging(PathBuf::from("bm78x-test.csv"), Box::new(out.clone()));
        assert!(app.log.is_some());

        app.on_update(frame_event(volts(32981)), 1.0, wall());
        // Logging continues while paused: pause is for the display.
        app.on_key(char_key('p'));
        app.on_update(frame_event(volts(32979)), 1.2, wall());

        assert_eq!(app.on_key(char_key('l')), None);
        assert!(app.log.is_none());
        let message = app.message.clone().unwrap();
        assert!(
            message.contains("bm78x-test.csv") && message.contains('2'),
            "{message}"
        );

        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), 3, "header and two rows:\n{text}");
        assert!(text.contains(",3.2979,"));
    }

    #[test]
    fn logging_needs_a_meter() {
        let (mut app, _) = App::new(None);
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.on_key(char_key('l')), None);
        assert!(app.message.as_deref().unwrap_or("").contains("meter"));
    }

    #[test]
    fn held_readings_are_not_counted() {
        // HOLD and A-HOLD repeat one reading; CREST, MAX, MIN and AVG show
        // the meter's own record of past ones.
        let mut app = connected();
        app.on_update(frame_event(volts(1000)), 1.0, wall());
        for status in [
            (0x30, 0),
            (0x18, 0),
            (0x90, 0),
            (0x10, 0x18),
            (0x10, 0x14),
            (0x10, 0x12),
        ] {
            app.on_update(
                frame_event(reading(2000, 1, Prefix::Base, Unit::Volt, status)),
                2.0,
                wall(),
            );
        }
        assert_eq!(app.stats.count(), 1);
        assert_eq!(app.trend.window(2.0, 10.0).len(), 1);
        assert_eq!(app.reading.unwrap().raw_value, 2000, "but they are shown");
        // RECORD alone shows the live reading.
        app.on_update(
            frame_event(reading(3000, 1, Prefix::Base, Unit::Volt, (0x10, 0x10))),
            3.0,
            wall(),
        );
        assert_eq!(app.stats.count(), 2);
    }

    #[test]
    fn rel_restarts_the_statistics() {
        // REL changes what the numbers mean, as a new function does.
        let mut app = connected();
        app.on_update(frame_event(volts(32981)), 1.0, wall());
        app.on_update(
            frame_event(reading(12, 1, Prefix::Base, Unit::Volt, (0x50, 0))),
            2.0,
            wall(),
        );
        assert_eq!(app.stats.count(), 1);
        assert_eq!(app.stats.max(), Some(0.0012));
        app.on_update(frame_event(volts(32981)), 3.0, wall());
        assert_eq!(app.stats.count(), 1);
    }

    #[test]
    fn link_states() {
        let mut app = connected();
        assert_eq!(app.link, Link::Connected);
        app.on_update(news(SessionEvent::Idle), 1.0, wall());
        assert_eq!(app.link, Link::Idle);
        app.on_update(frame_event(volts(1)), 2.0, wall());
        assert_eq!(app.link, Link::Connected);
        app.on_update(
            news(SessionEvent::Disconnected(DisconnectReason::LinkLost)),
            3.0,
            wall(),
        );
        assert!(matches!(app.link, Link::Retrying(_)), "{:?}", app.link);
        app.on_update(
            news(SessionEvent::ConnectFailed(SessionError::Timeout)),
            4.0,
            wall(),
        );
        assert_eq!(app.link, Link::Retrying(SessionError::Timeout.to_string()));
        app.on_update(
            Update::SessionFailed {
                session: 0,
                error: "the meter rejected the password".into(),
            },
            5.0,
            wall(),
        );
        assert_eq!(
            app.link,
            Link::Failed("the meter rejected the password".into())
        );
    }

    #[test]
    fn a_refused_clock_is_shown_and_the_meter_stays_connected() {
        let mut app = connected();
        let failure = Failure {
            command: 0x0010,
            code: 2,
        };
        app.on_update(news(SessionEvent::ClockRefused(failure)), 1.0, wall());
        assert_eq!(app.link, Link::Connected);
        let message = app.message.as_deref().unwrap_or_default();
        assert!(message.contains("clock was not set"), "{message}");
        assert!(message.contains("out of setting range"), "{message}");
    }

    #[test]
    fn a_meter_not_found_says_how_to_turn_its_bluetooth_on() {
        let (mut app, _) = App::new(Some(METER));
        let not_found = TransportError::from(BluezError::NotFound(METER));
        app.on_update(
            news(SessionEvent::ConnectFailed(not_found.into())),
            1.0,
            wall(),
        );
        let Link::Retrying(text) = &app.link else {
            panic!("expected Retrying, got {:?}", app.link);
        };
        assert!(
            text.starts_with(&format!("meter {METER} not found")),
            "{text}"
        );
        assert!(text.contains("hold Δ"), "{text}");
        // Other failures are shown as they are.
        app.on_update(
            news(SessionEvent::ConnectFailed(SessionError::Timeout)),
            2.0,
            wall(),
        );
        assert_eq!(app.link, Link::Retrying(SessionError::Timeout.to_string()));
    }

    #[test]
    fn lost_and_bad_frames_are_counted() {
        let mut app = connected();
        app.on_update(news(SessionEvent::FramesLost(3)), 1.0, wall());
        app.on_update(news(SessionEvent::FramesLost(2)), 1.0, wall());
        assert_eq!(app.lost, 5);
        let raw = vec![0xFF; 20];
        let error = Frame::parse(&raw).unwrap_err();
        app.on_update(news(SessionEvent::bad_frame(error, raw)), 1.0, wall());
        assert_eq!(app.bad_frames, 1);
        assert!(app.raw[0].decoded.contains("152"), "{}", app.raw[0].decoded);
    }

    #[test]
    fn small_mtu_is_warned_about() {
        let mut app = connected();
        app.on_update(
            Update::Mtu {
                session: 0,
                mtu: Some(REQUIRED_MTU),
            },
            0.0,
            wall(),
        );
        assert_eq!(app.message, None);
        app.on_update(
            Update::Mtu {
                session: 0,
                mtu: Some(23),
            },
            0.0,
            wall(),
        );
        let message = app.message.clone().unwrap();
        assert!(
            message.contains("23") && message.contains("185"),
            "{message}"
        );
    }
}
