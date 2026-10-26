//! The files the TUI makes: the log directory and the logs in it.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

/// `input` with a leading `~` replaced by `home`, as a shell would.
pub fn expand_home(input: &str, home: Option<&Path>) -> PathBuf {
    match (home, input.strip_prefix('~')) {
        (Some(home), Some("")) => home.to_path_buf(),
        (Some(home), Some(rest)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(input),
    }
}

/// The directory `typed` names, made absolute, or why it cannot hold logs.
pub fn resolve_log_dir(typed: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let dir = match expand_home(typed, home).canonicalize() {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err("it does not exist".into());
        }
        Err(error) => return Err(error.to_string()),
    };
    if !dir.is_dir() {
        return Err("not a directory".into());
    }
    // It is remembered as one line of UTF-8 text.
    match dir.to_str() {
        None => Err("its name is not UTF-8".into()),
        Some(name) if name.contains(['\n', '\r']) => Err("its name has a line break".into()),
        Some(_) => Ok(dir),
    }
}

/// Creates a new log in `dir`, named for `now`: `bm78x-YYYYMMDD-HHMMSS.csv`,
/// or with `-2`, `-3` and so on if that is taken. Never overwrites.
pub fn create_log(dir: &Path, now: DateTime<Local>) -> io::Result<(PathBuf, File)> {
    let stem = now.format("bm78x-%Y%m%d-%H%M%S").to_string();
    for n in 1.. {
        let name = match n {
            1 => format!("{stem}.csv"),
            n => format!("{stem}-{n}.csv"),
        };
        let path = dir.join(name);
        match File::create_new(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && n < 1000 => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("the loop returns by 1000")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bm78x-tui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn home_is_expanded() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(expand_home("~/logs", home), PathBuf::from("/home/me/logs"));
        assert_eq!(expand_home("~", home), PathBuf::from("/home/me"));
        assert_eq!(expand_home("/var/log", home), PathBuf::from("/var/log"));
        assert_eq!(expand_home("logs", home), PathBuf::from("logs"));
        assert_eq!(expand_home("~other/x", home), PathBuf::from("~other/x"));
        assert_eq!(expand_home("~/logs", None), PathBuf::from("~/logs"));
    }

    #[test]
    fn a_log_dir_is_made_absolute() {
        let dir = scratch_dir("resolve");
        std::fs::create_dir(dir.join("logs")).unwrap();
        let typed = format!("{}/logs/../logs", dir.display());
        let resolved = resolve_log_dir(&typed, None).unwrap();
        assert_eq!(resolved, dir.join("logs").canonicalize().unwrap());
        // `~` is the home directory.
        let home_typed = resolve_log_dir("~/logs", Some(&dir)).unwrap();
        assert_eq!(home_typed, resolved);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bad_log_dirs_say_why() {
        let dir = scratch_dir("bad");
        let file = dir.join("file");
        std::fs::write(&file, "").unwrap();
        let reason = |typed: &str| resolve_log_dir(typed, None).unwrap_err();
        assert_eq!(
            reason(&format!("{}/missing", dir.display())),
            "it does not exist"
        );
        assert_eq!(reason(&file.to_string_lossy()), "not a directory");
        // The state file holds one path per line.
        let newline = dir.join("two\nlines");
        std::fs::create_dir(&newline).unwrap();
        assert_eq!(
            reason(&newline.to_string_lossy()),
            "its name has a line break"
        );
        // Nor can it hold a name that is not UTF-8, here reached by a link.
        use std::os::unix::ffi::OsStrExt;
        let odd = dir.join(std::ffi::OsStr::from_bytes(b"\xff"));
        std::fs::create_dir(&odd).unwrap();
        std::os::unix::fs::symlink(&odd, dir.join("link")).unwrap();
        let link = dir.join("link");
        assert_eq!(reason(&link.to_string_lossy()), "its name is not UTF-8");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn logs_never_overwrite() {
        let dir = scratch_dir("logs");
        let now = Local.with_ymd_and_hms(2026, 10, 24, 9, 30, 5).unwrap();
        let names: Vec<_> = (0..3)
            .map(|_| {
                let (path, _file) = create_log(&dir, now).unwrap();
                assert_eq!(path.parent(), Some(dir.as_path()));
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "bm78x-20261024-093005.csv",
                "bm78x-20261024-093005-2.csv",
                "bm78x-20261024-093005-3.csv",
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_log_dir_is_an_error() {
        let dir = scratch_dir("gone").join("gone");
        let now = Local.with_ymd_and_hms(2026, 10, 24, 9, 30, 5).unwrap();
        let error = create_log(&dir, now).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
