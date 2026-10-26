//! Text for readings: what the LCD shows, units, flags, and exact values.

use bm78x::protocol::{Flags, Number, Prefix, Reading, Readout};

/// The display as the LCD shows it, without the unit: `3.2981`, `-0.0012`,
/// `OL`, `Auto`.
pub fn display_text(reading: &Reading) -> String {
    match reading.readout() {
        Readout::Number(number) => number.to_string(),
        Readout::Overload => "OL".into(),
        Readout::Text(text) => text.to_string(),
        // Whatever the library adds later, as it formats it.
        _ => reading.to_string(),
    }
}

/// The prefix and unit: `V`, `mV`, `°C`.
pub fn unit_text(reading: &Reading) -> String {
    format!("{}{}", reading.prefix, reading.unit)
}

/// A value in base units shown as the LCD would show it in `reading`'s
/// prefix, decimals and unit, such as `3.2981 V`. `None` if the prefix is
/// unknown.
pub fn in_display_units(si: f64, reading: &Reading) -> Option<String> {
    let value = si / reading.prefix.multiplier()?;
    let decimals = usize::from(reading.decimals());
    let text = format!("{value:.decimals$}");
    // A computed value that rounds to zero is zero, whatever its sign.
    let text = match text.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => text,
    };
    Some(format!("{text} {}", unit_text(reading)))
}

/// A displayed number in base units, as exact decimal text: 3.2981 mV is
/// `0.0032981`. `None` if the prefix is unknown. Computed from the digits,
/// so it carries no floating-point noise.
pub fn si_text(number: &Number, prefix: Prefix) -> Option<String> {
    prefix.multiplier()?;
    // Moving to base units shifts the decimal point by the prefix's power
    // of ten: 3.2981 mV has 4 + 3 = 7 decimals in volts.
    let decimals = i32::from(number.decimals) - i32::from(prefix.exponent());
    let digits = number.magnitude.to_string();
    let unsigned = if decimals <= 0 {
        format!("{digits}{}", "0".repeat(decimals.unsigned_abs() as usize))
    } else {
        let decimals = decimals as usize;
        let padded = format!("{digits:0>width$}", width = decimals + 1);
        let (whole, fraction) = padded.split_at(padded.len() - decimals);
        format!("{whole}.{fraction}")
    };
    let sign = if number.negative { "-" } else { "" };
    Some(format!("{sign}{unsigned}"))
}

/// The names of the annunciators that are on, in a fixed order.
pub fn flag_names(flags: &Flags) -> Vec<&'static str> {
    let annunciators: [(bool, &str); 9] = [
        (flags.auto_range(), "AUTO"),
        (flags.hold(), "HOLD"),
        (flags.auto_hold(), "A-HOLD"),
        (flags.relative(), "REL"),
        (flags.crest(), "CREST"),
        (flags.record(), "REC"),
        (flags.max(), "MAX"),
        (flags.min(), "MIN"),
        (flags.avg(), "AVG"),
    ];
    annunciators
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bm78x::protocol::{Function, MeterTime, Unit};

    /// A reading with the given wire fields; everything else is DC volts.
    pub fn reading(
        raw_value: i32,
        decimal_point: u8,
        prefix: Prefix,
        unit: Unit,
        status: (u8, u8),
    ) -> Reading {
        Reading {
            logging_set_id: 1,
            packet_id: 1,
            time: MeterTime::default(),
            flags: Flags {
                status0: status.0,
                status1: status.1,
                status2: 0,
            },
            device_type: 1,
            function: Function::DcVolts,
            raw_value,
            decimal_point,
            prefix,
            unit,
            digits: 5,
        }
    }

    fn volts(raw_value: i32) -> Reading {
        reading(raw_value, 1, Prefix::Base, Unit::Volt, (0x10, 0))
    }

    #[test]
    fn display_text_matches_a_real_lcd() {
        // Frames from a BM787BT, and what its LCD showed for each; the same
        // capture as `displays` in the bm78x crate's tests/captures.rs.
        let capture = include_str!("../tests/captures/bm787bt-displays.hex");
        let lcd = [
            "Auto", "-3.4480", "2.06", "OL", "OL", "OL", "----", "----", "0.0001", "InEr", "EF-H",
            "EF-L", "-----",
        ];
        let lines: Vec<&str> = capture.lines().collect();
        assert_eq!(lines.len(), lcd.len());
        for (line, lcd) in lines.into_iter().zip(lcd) {
            let bytes: Vec<u8> = (0..line.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
                .collect();
            let frame = bm78x::protocol::Frame::parse(&bytes).unwrap();
            assert_eq!(display_text(frame.reading().unwrap()), lcd);
        }
    }

    #[test]
    fn display_text_matches_the_lcd() {
        assert_eq!(display_text(&volts(32981)), "3.2981");
        assert_eq!(
            display_text(&reading(12, 1, Prefix::Base, Unit::Volt, (0, 0x40))),
            "-0.0012"
        );
        assert_eq!(
            display_text(&reading(0, 1, Prefix::Base, Unit::Ohm, (0, 0x20))),
            "OL"
        );
        assert_eq!(
            display_text(&reading(1, 1, Prefix::Base, Unit::Volt, (0x04, 0))),
            "Auto"
        );
    }

    #[test]
    fn unit_text_has_prefix_and_unit() {
        assert_eq!(unit_text(&volts(1)), "V");
        assert_eq!(
            unit_text(&reading(1, 1, Prefix::Milli, Unit::Volt, (0, 0))),
            "mV"
        );
        assert_eq!(
            unit_text(&reading(1, 1, Prefix::Micro, Unit::Ampere, (0, 0))),
            "μA"
        );
        assert_eq!(
            unit_text(&reading(1, 1, Prefix::Base, Unit::Celsius, (0, 0))),
            "°C"
        );
    }

    #[test]
    fn statistics_that_round_to_zero_have_no_sign() {
        // Seen on a BM787BT: the meter's negative zeros made MAX read
        // -0.0000 V above an AVG of 0.0000 V.
        assert_eq!(
            in_display_units(-0.0, &volts(1)).as_deref(),
            Some("0.0000 V")
        );
        assert_eq!(
            in_display_units(-0.00004, &volts(1)).as_deref(),
            Some("0.0000 V")
        );
        assert_eq!(
            in_display_units(-0.0001, &volts(1)).as_deref(),
            Some("-0.0001 V")
        );
    }

    #[test]
    fn values_in_display_units() {
        // 5 digits, dp 1: four decimals.
        assert_eq!(
            in_display_units(3.3021, &volts(1)).as_deref(),
            Some("3.3021 V")
        );
        assert_eq!(
            in_display_units(-1.234, &volts(1)).as_deref(),
            Some("-1.2340 V")
        );
        // 5 digits, dp 3 in mV: two decimals.
        let millivolts = reading(1, 3, Prefix::Milli, Unit::Volt, (0, 0));
        assert_eq!(
            in_display_units(0.0123456, &millivolts).as_deref(),
            Some("12.35 mV")
        );
        let unknown = reading(1, 1, Prefix::from_exponent(2), Unit::Volt, (0, 0));
        assert_eq!(in_display_units(1.0, &unknown), None);
    }

    #[test]
    fn exact_si_text() {
        let n = |magnitude, negative, decimals| Number {
            magnitude,
            negative,
            decimals,
        };
        assert_eq!(
            si_text(&n(32981, false, 4), Prefix::Base).as_deref(),
            Some("3.2981")
        );
        assert_eq!(
            si_text(&n(32981, false, 4), Prefix::Milli).as_deref(),
            Some("0.0032981")
        );
        assert_eq!(
            si_text(&n(32981, true, 4), Prefix::Micro).as_deref(),
            Some("-0.0000032981")
        );
        assert_eq!(
            si_text(&n(12345, false, 3), Prefix::Kilo).as_deref(),
            Some("12345")
        );
        assert_eq!(
            si_text(&n(5, false, 0), Prefix::Mega).as_deref(),
            Some("5000000")
        );
        assert_eq!(
            si_text(&n(15, false, 1), Prefix::Kilo).as_deref(),
            Some("1500")
        );
        assert_eq!(
            si_text(&n(0, false, 4), Prefix::Base).as_deref(),
            Some("0.0000")
        );
        assert_eq!(si_text(&n(1, false, 0), Prefix::from_exponent(2)), None);
    }

    #[test]
    fn flag_names_in_order() {
        let flags = |status0, status1| Flags {
            status0,
            status1,
            status2: 0,
        };
        assert!(flag_names(&flags(0, 0)).is_empty());
        assert_eq!(flag_names(&flags(0x10, 0)), ["AUTO"]);
        assert_eq!(
            flag_names(&flags(0xFC, 0x1E)),
            [
                "AUTO", "HOLD", "A-HOLD", "REL", "CREST", "REC", "MAX", "MIN", "AVG"
            ]
        );
        // Sign, OL and ASCII are part of the display, not annunciators.
        assert!(flag_names(&flags(0x04, 0x60)).is_empty());
    }
}
