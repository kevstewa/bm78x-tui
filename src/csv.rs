//! CSV logging, one row per frame.

use std::io::{self, Write};

use bm78x::protocol::{Reading, Readout};
use chrono::{DateTime, FixedOffset};

use crate::format::{display_text, flag_names, si_text};

/// The header row.
pub const HEADER: &str = "host_time,meter_time,function,display,prefix,unit,si_value,flags";

/// A CSV log being written.
pub struct CsvLog<W: Write> {
    out: W,
    rows: u64,
}

impl<W: Write> CsvLog<W> {
    /// Starts a log by writing the header.
    pub fn new(mut out: W) -> io::Result<Self> {
        writeln!(out, "{HEADER}")?;
        out.flush()?;
        Ok(CsvLog { out, rows: 0 })
    }

    /// Writes one reading received at `host_time`, and flushes, so a crash
    /// loses nothing already received.
    ///
    /// Columns: the host's time (RFC 3339, milliseconds), the meter's clock,
    /// the function, the display as the LCD shows it, the prefix, the unit,
    /// the value in base units (empty unless numeric), and the annunciators
    /// that are on, separated by spaces.
    pub fn write(&mut self, host_time: DateTime<FixedOffset>, reading: &Reading) -> io::Result<()> {
        let t = reading.time;
        let si_value = match reading.readout() {
            Readout::Number(number) => si_text(&number, reading.prefix).unwrap_or_default(),
            _ => String::new(),
        };
        let fields = [
            host_time.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string(),
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
                t.year, t.month, t.day, t.hour, t.minute, t.second, t.millisecond
            ),
            reading.function.to_string(),
            display_text(reading),
            reading.prefix.to_string(),
            reading.unit.to_string(),
            si_value,
            flag_names(&reading.flags).join(" "),
        ];
        let row: Vec<String> = fields.iter().map(|f| quote(f)).collect();
        writeln!(self.out, "{}", row.join(","))?;
        self.out.flush()?;
        self.rows += 1;
        Ok(())
    }

    /// How many rows have been written, not counting the header.
    pub fn rows(&self) -> u64 {
        self.rows
    }
}

/// Quotes a field if it holds a comma, quote or line break (RFC 4180).
fn quote(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::tests::reading;
    use bm78x::protocol::{Function, MeterTime, Prefix, Unit};

    fn host_time() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-10-23T14:05:01.234+02:00").unwrap()
    }

    fn rows(readings: &[Reading]) -> Vec<String> {
        let mut log = CsvLog::new(Vec::new()).unwrap();
        for r in readings {
            log.write(host_time(), r).unwrap();
        }
        assert_eq!(log.rows(), readings.len() as u64);
        let text = String::from_utf8(log.out).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        assert_eq!(lines.remove(0), HEADER);
        assert!(text.ends_with('\n'));
        lines
    }

    #[test]
    fn numeric_row() {
        let mut r = reading(32981, 1, Prefix::Base, Unit::Volt, (0x10, 0));
        r.time = MeterTime {
            year: 2023,
            month: 2,
            day: 8,
            hour: 0,
            minute: 1,
            second: 41,
            millisecond: 863,
        };
        assert_eq!(
            rows(&[r]),
            ["2026-10-23T14:05:01.234+02:00,2023-02-08T00:01:41.863,DCV,3.2981,,V,3.2981,AUTO"]
        );
    }

    #[test]
    fn prefixed_negative_row() {
        let r = reading(12345, 2, Prefix::Milli, Unit::Volt, (0x30, 0x40));
        let row = &rows(&[r])[0];
        assert!(
            row.ends_with(",DCV,-12.345,m,V,-0.012345,AUTO HOLD"),
            "{row}"
        );
    }

    #[test]
    fn overload_has_no_value() {
        let mut r = reading(0, 1, Prefix::Mega, Unit::Ohm, (0x10, 0x20));
        r.function = Function::Resistance;
        let row = &rows(&[r])[0];
        assert!(row.ends_with(",Resistance,OL,M,Ω,,AUTO"), "{row}");
    }

    #[test]
    fn fields_with_commas_are_quoted() {
        let mut r = reading(1, 1, Prefix::Base, Unit::Volt, (0, 0));
        r.function = Function::from_ids(0x02, 0x02);
        let row = &rows(&[r])[0];
        assert!(row.contains(",\"function(0x02, 0x02)\","), "{row}");
    }

    struct CountFlushes {
        flushes: usize,
    }

    impl Write for CountFlushes {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn every_row_is_flushed() {
        let mut log = CsvLog::new(CountFlushes { flushes: 0 }).unwrap();
        let after_header = log.out.flushes;
        let r = reading(1, 1, Prefix::Base, Unit::Volt, (0, 0));
        log.write(host_time(), &r).unwrap();
        log.write(host_time(), &r).unwrap();
        assert_eq!(log.out.flushes, after_header + 2);
    }
}
