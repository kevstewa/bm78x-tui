//! A terminal UI for Brymen BM78xBT multimeters.

mod app;
mod bigtext;
mod csv;
mod files;
mod format;
mod saved;
mod stats;
mod tasks;
mod trend;
mod ui;

use std::io::{BufWriter, stdout};
use std::path::PathBuf;
use std::time::Duration;

use bm78x::bluez::{Bluez, BluezConfig, BluezTransport};
use bm78x::protocol::{MacAddress, Password, RtcTime, Weekday};
use bm78x::session::{Clock, Session, SessionConfig, SessionEvent};
use chrono::{DateTime, Datelike, FixedOffset, Local, Timelike};
use clap::Parser;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind,
};
use crossterm::execute;
use futures_util::StreamExt;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

use app::{Action, App, Update};
use files::{create_log, resolve_log_dir};
use saved::Saved;
use tasks::{Stop, Tasks, Which};

/// Live readings, trend and logging for a Brymen BM78xBT multimeter.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Connect to this meter instead of choosing one from a scan.
    #[arg(long)]
    address: Option<MacAddress>,
    /// The meter's connection password.
    #[arg(long, default_value = "0000")]
    password: Password,
    /// Where to write CSV logs, for this run. Without it, the directory set
    /// in the TUI, or else the current directory.
    #[arg(long)]
    log_dir: Option<PathBuf>,
}

/// How long quitting waits for the meter to be released. BlueZ takes about
/// three seconds to disconnect a BM787BT.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(6);

/// News from a task, with when it arrived.
struct Stamped {
    update: Update,
    at: Instant,
    wall: DateTime<FixedOffset>,
}

type Updates = mpsc::UnboundedSender<Stamped>;

fn send(updates: &Updates, update: Update) {
    let _ = updates.send(Stamped {
        update,
        at: Instant::now(),
        wall: Local::now().fixed_offset(),
    });
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    // Fail before taking over the terminal if BlueZ is not there.
    let bluez = Bluez::system().await?;
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    let mut tasks = Tasks::default();
    let result = run(&mut terminal, &mut tasks, bluez, args).await;
    // Release the meter whatever went wrong: it takes one connection at a
    // time, and BlueZ would otherwise keep it.
    let _ = timeout(RELEASE_TIMEOUT, tasks.stop_all()).await;
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    tasks: &mut Tasks,
    bluez: Bluez,
    args: Args,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let seconds = move |at: Instant| at.duration_since(start).as_secs_f64();
    let (updates, mut received) = mpsc::unbounded_channel();
    // Closing the terminal sends SIGHUP; `kill` and logging out, SIGTERM.
    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;

    let (mut app, mut action) = App::new(args.address);
    let mut saved_path = saved::default_path();
    match saved_path.as_deref().map(Saved::load) {
        Some(Ok(saved)) => app.saved = saved,
        Some(Err(error)) => {
            // Saving now could overwrite what could not be read.
            app.message = Some(format!("Cannot read saved meters: {error}"));
            saved_path = None;
        }
        None => app.message = Some("No home directory: nothing will be remembered".into()),
    }
    app.log_dir = args
        .log_dir
        .clone()
        .or_else(|| app.saved.log_dir.clone())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut keys = EventStream::new();
    // Redraw at least this often, so the plot scrolls during silence.
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        match action.take() {
            None => {}
            Some(Action::Quit) => break,
            Some(Action::StartScan) => {
                let (bluez, updates) = (bluez.clone(), updates.clone());
                tasks
                    .start(Which::Scan, move |stop| scan(bluez, updates, stop))
                    .await;
            }
            Some(Action::StopScan) => tasks.stop(Which::Scan).await,
            Some(Action::Connect(address)) => {
                tasks.stop(Which::Scan).await;
                let meter = bluez.meter(address, BluezConfig::default());
                let (id, password, updates) = (app.session, args.password, updates.clone());
                tasks
                    .start(Which::Session, move |stop| {
                        run_session(meter, password, id, updates, stop)
                    })
                    .await;
            }
            Some(Action::StartLogging) => match create_log(&app.log_dir, Local::now()) {
                Ok((path, file)) => app.start_logging(path, Box::new(BufWriter::new(file))),
                Err(error) => {
                    let dir = app.log_dir.display();
                    app.message = Some(format!("Cannot start a log in {dir}: {error}"));
                }
            },
            Some(Action::SetLogDir(input)) => {
                let typed = input.to_string_lossy().into_owned();
                match resolve_log_dir(&typed, std::env::home_dir().as_deref()) {
                    Ok(dir) => app.set_log_dir(dir),
                    Err(reason) => app.reject_log_dir(typed, &reason),
                }
            }
        }
        if !app.saved_changes.is_empty() {
            let changes = std::mem::take(&mut app.saved_changes);
            if let Some(path) = &saved_path {
                match saved::update(path, &changes) {
                    Ok(saved) => app.saved = saved,
                    Err(error) => {
                        app.message = Some(format!("Cannot save to {}: {error}", path.display()))
                    }
                }
            }
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;
        action = tokio::select! {
            event = keys.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => app.on_key(key),
                Some(Ok(Event::Paste(text))) => {
                    app.on_paste(&text);
                    None
                }
                Some(Ok(_)) => None,
                Some(Err(error)) => return Err(error.into()),
                None => Some(Action::Quit),
            },
            Some(Stamped { update, at, wall }) = received.recv() => {
                app.on_update(update, seconds(at), wall);
                None
            }
            (which, result) = tasks.ended() => match result {
                Err(error) if error.is_panic() => {
                    return Err(format!("the {which} task failed: {error}").into());
                }
                // A scan or session that ends by itself has said why.
                _ => None,
            },
            _ = tick.tick() => {
                app.tick(seconds(Instant::now()));
                None
            }
            _ = hangup.recv() => Some(Action::Quit),
            _ = terminate.recv() => Some(Action::Quit),
        };
    }
    app.message = Some("Releasing the meter…".into());
    let _ = terminal.draw(|frame| ui::draw(frame, &app));
    Ok(())
}

/// Scans until told to stop, sending each meter seen.
async fn scan(bluez: Bluez, updates: Updates, mut stop: Stop) {
    let mut scan = match bluez.scan().await {
        Ok(scan) => scan,
        Err(error) => {
            send(&updates, Update::ScanFailed(error.to_string()));
            return;
        }
    };
    loop {
        tokio::select! {
            _ = &mut stop => break,
            found = scan.next() => match found {
                Ok(found) => send(&updates, Update::Discovered(found)),
                Err(error) => {
                    send(&updates, Update::ScanFailed(error.to_string()));
                    break;
                }
            },
        }
    }
    let _ = scan.stop().await;
}

/// Runs session number `id` until told to stop, sending everything it
/// reports.
async fn run_session(
    meter: BluezTransport,
    password: Password,
    id: u64,
    updates: Updates,
    mut stop: Stop,
) {
    let mut config = SessionConfig::default();
    config.password = password;
    config.clock = Some(Clock::new(local_time));
    let mut session = Session::new(meter, config);
    loop {
        // Stopping mid-connect leaves the link half made; `close` below
        // sorts that out.
        let event = tokio::select! {
            _ = &mut stop => break,
            event = session.next_event() => event,
        };
        match event {
            Ok(event) => {
                let connected = matches!(event, SessionEvent::Connected { .. });
                send(&updates, Update::Session { session: id, event });
                if connected {
                    let mtu = session.transport().mtu();
                    send(&updates, Update::Mtu { session: id, mtu });
                }
            }
            Err(error) => {
                let error = error.to_string();
                send(&updates, Update::SessionFailed { session: id, error });
                break;
            }
        }
    }
    // Release the meter: it accepts one connection at a time.
    let _ = session.close().await;
}

/// The host's local time, for setting the meter's clock.
fn local_time() -> Option<RtcTime> {
    rtc_time(Local::now())
}

/// `time` as the meter takes it, or `None` outside the years it takes.
fn rtc_time<T: Datelike + Timelike>(time: T) -> Option<RtcTime> {
    let weekday = match time.weekday() {
        chrono::Weekday::Mon => Weekday::Monday,
        chrono::Weekday::Tue => Weekday::Tuesday,
        chrono::Weekday::Wed => Weekday::Wednesday,
        chrono::Weekday::Thu => Weekday::Thursday,
        chrono::Weekday::Fri => Weekday::Friday,
        chrono::Weekday::Sat => Weekday::Saturday,
        chrono::Weekday::Sun => Weekday::Sunday,
    };
    // chrono's fields are all in range, so only the year can be refused.
    RtcTime::new(
        u16::try_from(time.year()).ok()?,
        time.month() as u8,
        time.day() as u8,
        weekday,
        time.hour() as u8,
        time.minute() as u8,
        time.second() as u8,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn clock_fields_and_weekdays() {
        let at = |year, day, h, m, s| {
            NaiveDate::from_ymd_opt(year, 10, day)
                .unwrap()
                .and_hms_opt(h, m, s)
                .unwrap()
        };
        // 2026-10-25 is a Sunday, the 26th a Monday.
        let sunday = rtc_time(at(2026, 25, 23, 59, 58)).unwrap();
        assert_eq!(
            (
                sunday.year(),
                sunday.month(),
                sunday.day(),
                sunday.weekday()
            ),
            (2026, 10, 25, Weekday::Sunday)
        );
        assert_eq!(
            (sunday.hour(), sunday.minute(), sunday.second()),
            (23, 59, 58)
        );
        let monday = rtc_time(at(2026, 26, 0, 0, 0)).unwrap();
        assert_eq!(monday.weekday(), Weekday::Monday);
        // The meter takes only 2000 to 2098; outside them its clock is left.
        assert!(rtc_time(at(2098, 1, 0, 0, 0)).is_some());
        assert_eq!(rtc_time(at(2099, 1, 0, 0, 0)), None);
        assert_eq!(rtc_time(at(1999, 1, 0, 0, 0)), None);
    }
}
