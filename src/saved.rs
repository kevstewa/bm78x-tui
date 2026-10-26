//! What the TUI remembers between runs: the meters it has connected to and
//! the log directory. Kept as lines of text, newest meter first:
//!
//! ```text
//! log-dir /home/me/logs
//! meter 00:11:22:33:44:55 BM78xBT
//! ```

use std::io;
use std::path::{Path, PathBuf};

use bm78x::protocol::MacAddress;

/// One change to what is remembered. Programs save their changes, not
/// their whole copy, so two running at once keep each other's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Connected to a meter, which goes first; see [`Saved::remember`].
    Remember(MacAddress, Option<String>),
    /// Forget a meter.
    Forget(MacAddress),
    /// Set the log directory.
    LogDir(PathBuf),
}

/// Applies `changes` to the file at `path`, as it is now, and returns the
/// result.
///
/// A lock file beside it keeps two programs from doing this at once.
pub fn update(path: &Path, changes: &[Change]) -> io::Result<Saved> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    let lock = std::fs::File::create(lock)?;
    lock.lock()?;
    let mut saved = Saved::load(path)?;
    for change in changes {
        saved.apply(change);
    }
    saved.save(path)?;
    Ok(saved)
}

/// A meter connected to before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedMeter {
    /// Its address.
    pub address: MacAddress,
    /// Its name when last seen, if it advertised one.
    pub name: Option<String>,
}

/// Everything remembered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saved {
    /// Meters connected to, most recent first.
    pub meters: Vec<SavedMeter>,
    /// The log directory set in the TUI.
    pub log_dir: Option<PathBuf>,
}

impl Saved {
    /// Reads the text form. Lines it does not understand are skipped, so a
    /// damaged file loses only those lines.
    #[cfg(test)]
    pub fn parse(text: &str) -> Self {
        Self::parse_lines(text.lines())
    }

    /// Reads the text form, a line at a time. Lines it does not understand
    /// are skipped, so a damaged file loses only those lines.
    fn parse_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut saved = Saved::default();
        for line in lines {
            if let Some(dir) = line.strip_prefix("log-dir ") {
                saved.log_dir = Some(PathBuf::from(dir));
            } else if let Some(rest) = line.strip_prefix("meter ") {
                let (address, name) = match rest.split_once(' ') {
                    Some((address, name)) => (address, Some(name.to_string())),
                    None => (rest, None),
                };
                let Ok(address) = address.parse() else {
                    continue;
                };
                if !saved.meters.iter().any(|m| m.address == address) {
                    saved.meters.push(SavedMeter { address, name });
                }
            }
        }
        saved
    }

    /// The text form.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        if let Some(dir) = &self.log_dir {
            text.push_str(&format!("log-dir {}\n", dir.display()));
        }
        for meter in &self.meters {
            match &meter.name {
                Some(name) => text.push_str(&format!("meter {} {name}\n", meter.address)),
                None => text.push_str(&format!("meter {}\n", meter.address)),
            }
        }
        text
    }

    /// Makes one change.
    pub fn apply(&mut self, change: &Change) {
        match change {
            Change::Remember(address, name) => {
                // A meter can advertise any name; keep it to one line.
                let name = name.as_ref().map(|name| {
                    name.chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect()
                });
                self.remember(*address, name);
            }
            Change::Forget(address) => self.forget(*address),
            Change::LogDir(dir) => self.log_dir = Some(dir.clone()),
        }
    }

    /// Records a connection to `address`: it moves to the front, keeping
    /// its old name if `name` is `None`.
    pub fn remember(&mut self, address: MacAddress, name: Option<String>) {
        let old = self
            .meters
            .iter()
            .position(|m| m.address == address)
            .map(|i| self.meters.remove(i));
        let name = name.or(old.and_then(|m| m.name));
        self.meters.insert(0, SavedMeter { address, name });
    }

    /// Forgets `address`.
    pub fn forget(&mut self, address: MacAddress) {
        self.meters.retain(|m| m.address != address);
    }

    /// Reads `path`. A missing file is an empty `Saved`. Lines that are not
    /// UTF-8 are skipped, like any other damaged line.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let lines = bytes
                    .split(|&b| b == b'\n')
                    .filter_map(|line| std::str::from_utf8(line).ok())
                    .map(|line| line.strip_suffix('\r').unwrap_or(line));
                Ok(Saved::parse_lines(lines))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Saved::default()),
            Err(error) => Err(error),
        }
    }

    /// Writes `path`, creating its directory. The file is replaced whole,
    /// and synced first, so a crash leaves either the old or the new one.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let unsafe_text = |s: &str| s.contains(['\n', '\r']);
        let dir_bad = self.log_dir.as_ref().is_some_and(|d| match d.to_str() {
            Some(d) => unsafe_text(d),
            None => true,
        });
        let name_bad = self
            .meters
            .iter()
            .any(|m| m.name.as_deref().is_some_and(unsafe_text));
        if dir_bad || name_bad {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a path or name has a line break or is not UTF-8",
            ));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(format!(".{}.new", std::process::id()));
        let mut file = std::fs::File::create(&temporary)?;
        std::io::Write::write_all(&mut file, self.to_text().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        // The rename is durable once the directory is synced too.
        if let Some(dir) = path.parent()
            && let Ok(dir) = std::fs::File::open(dir)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

/// Where the state lives: `$XDG_STATE_HOME/bm78x-tui/saved`, or
/// `~/.local/state/bm78x-tui/saved`. `None` without either variable.
pub fn default_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".local").join("state")))?;
    Some(state.join("bm78x-tui").join("saved"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: MacAddress = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    const B: MacAddress = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x66]);

    fn meter(address: MacAddress, name: Option<&str>) -> SavedMeter {
        SavedMeter {
            address,
            name: name.map(String::from),
        }
    }

    #[test]
    fn reads_the_documented_format() {
        let text = "log-dir /home/me/my logs\n\
                    meter 00:11:22:33:44:55 BM78xBT\n\
                    meter 00:11:22:33:44:66\n";
        let saved = Saved::parse(text);
        assert_eq!(saved.log_dir, Some(PathBuf::from("/home/me/my logs")));
        assert_eq!(
            saved.meters,
            vec![meter(A, Some("BM78xBT")), meter(B, None)]
        );
    }

    #[test]
    fn names_may_have_spaces() {
        let saved = Saved::parse("meter 00:11:22:33:44:55 Bench meter 2\n");
        assert_eq!(saved.meters, vec![meter(A, Some("Bench meter 2"))]);
    }

    #[test]
    fn text_round_trips() {
        let saved = Saved {
            meters: vec![meter(B, Some("Bench meter")), meter(A, None)],
            log_dir: Some(PathBuf::from("/tmp/logs")),
        };
        assert_eq!(Saved::parse(&saved.to_text()), saved);
        assert_eq!(Saved::parse(&Saved::default().to_text()), Saved::default());
    }

    #[test]
    fn damaged_lines_are_skipped() {
        let text = "meter not-an-address\n\
                    something else\n\
                    \n\
                    meter 00:11:22:33:44:55\n\
                    meter 00:11:22:33:44:55 duplicate\n";
        assert_eq!(Saved::parse(text).meters, vec![meter(A, None)]);
    }

    #[test]
    fn remember_puts_the_meter_first_once() {
        let mut saved = Saved::default();
        saved.remember(A, Some("BM78xBT".into()));
        saved.remember(B, None);
        assert_eq!(
            saved.meters,
            vec![meter(B, None), meter(A, Some("BM78xBT"))]
        );
        // Connecting by address gives no name; the old one stays.
        saved.remember(A, None);
        assert_eq!(
            saved.meters,
            vec![meter(A, Some("BM78xBT")), meter(B, None)]
        );
        saved.remember(A, Some("Renamed".into()));
        assert_eq!(saved.meters[0], meter(A, Some("Renamed")));
        assert_eq!(saved.meters.len(), 2);
    }

    #[test]
    fn forget_removes_the_meter() {
        let mut saved = Saved::default();
        saved.remember(A, None);
        saved.remember(B, None);
        saved.forget(A);
        assert_eq!(saved.meters, vec![meter(B, None)]);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bm78x-tui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = scratch_dir("missing");
        assert_eq!(Saved::load(&dir.join("saved")).unwrap(), Saved::default());
    }

    #[test]
    fn save_creates_the_directory_and_load_reads_it_back() {
        let dir = scratch_dir("save");
        let path = dir.join("nested").join("saved");
        let mut saved = Saved::default();
        saved.remember(A, Some("BM78xBT".into()));
        saved.log_dir = Some(PathBuf::from("/tmp"));
        saved.save(&path).unwrap();
        assert_eq!(Saved::load(&path).unwrap(), saved);

        // Saving again replaces the file rather than appending.
        saved.forget(A);
        saved.save(&path).unwrap();
        assert_eq!(Saved::load(&path).unwrap(), saved);
        // No temporary file is left behind.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["saved"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lines_that_are_not_utf8_are_skipped() {
        // Not the whole file: that would lose every meter on the next save.
        let dir = scratch_dir("utf8");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("saved");
        let mut bytes = b"meter 00:11:22:33:44:55 Bench\n".to_vec();
        bytes.extend(b"log-dir /tmp/\xff\xfe\n");
        bytes.extend(b"meter 00:11:22:33:44:66\n");
        std::fs::write(&path, bytes).unwrap();
        let saved = Saved::load(&path).unwrap();
        assert_eq!(saved.meters, vec![meter(A, Some("Bench")), meter(B, None)]);
        assert_eq!(saved.log_dir, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_are_kept_on_one_line() {
        // A meter can advertise any name; one with a line break would
        // otherwise make every later save fail.
        let mut saved = Saved::default();
        saved.apply(&Change::Remember(A, Some("Bench\nmeter\t2".into())));
        assert_eq!(saved.meters, vec![meter(A, Some("Bench meter 2"))]);
    }

    #[test]
    fn changes_apply_in_order() {
        let mut saved = Saved::default();
        for change in [
            Change::Remember(A, Some("Bench".into())),
            Change::Remember(B, None),
            Change::LogDir(PathBuf::from("/tmp")),
            Change::Forget(A),
        ] {
            saved.apply(&change);
        }
        assert_eq!(saved.meters, vec![meter(B, None)]);
        assert_eq!(saved.log_dir, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn two_programs_keep_each_others_changes() {
        // Each runs `update` with only its own changes, against the file as
        // it is now, so neither overwrites what the other saved.
        let dir = scratch_dir("two");
        let path = dir.join("saved");
        let first = update(&path, &[Change::Remember(A, Some("Bench".into()))]).unwrap();
        assert_eq!(first.meters, vec![meter(A, Some("Bench"))]);
        let second = update(&path, &[Change::Remember(B, None)]).unwrap();
        assert_eq!(second.meters, vec![meter(B, None), meter(A, Some("Bench"))]);
        // The first program reconnects to A; B stays.
        let first = update(&path, &[Change::Remember(A, None)]).unwrap();
        assert_eq!(first.meters, vec![meter(A, Some("Bench")), meter(B, None)]);
        assert_eq!(Saved::load(&path).unwrap(), first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_updates_lose_nothing() {
        let dir = scratch_dir("concurrent");
        let path = dir.join("saved");
        let threads: Vec<_> = (0..8u8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let address = MacAddress([0, 0x11, 0x22, 0x33, 0x44, n]);
                    update(&path, &[Change::Remember(address, None)]).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(Saved::load(&path).unwrap().meters.len(), 8);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_path_with_a_newline_cannot_be_saved() {
        // It would break the line format.
        let saved = Saved {
            meters: Vec::new(),
            log_dir: Some(PathBuf::from("/tmp/a\nmeter 00:11:22:33:44:55")),
        };
        let dir = scratch_dir("newline");
        assert!(saved.save(&dir.join("saved")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
