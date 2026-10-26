# bm78x-tui

A terminal UI for the Brymen BM787BT and BM788BT Bluetooth multimeters:
live readings, statistics, a trend plot and CSV logging, in pure Rust on
Linux. It is built on the [bm78x](https://github.com/kevstewa/bm78x)
library.

![bm78x-tui showing DC volts, with statistics and a trend plot](https://github.com/kevstewa/bm78x-tui/raw/main/docs/screenshot.png)

## Requirements

Linux with BlueZ running (`systemctl enable --now bluetooth`) and a
Bluetooth LE adapter. No C libraries are needed. The meter accepts one
connection at a time, so close Brymen's phone app first.

To turn the meter's Bluetooth on, hold Δ for a second until the Bluetooth
annunciator lights.

## Use

```sh
cargo install --locked bm78x-tui
bm78x-tui
```

It opens with a menu of meters: the ones connected to before, then any others
in range. `m` brings the menu back to switch meters, and `--address` skips it.
The screen shows the reading in large digits with the meter's annunciators,
minimum, maximum and average, and a trend plot (`r` swaps it for the raw
frames), and logs to CSV. Held readings, and the meter's own CREST, MAX, MIN
and AVG, are shown but left out of the statistics and plot. `d` sets the log
directory. The keys are listed at the bottom of the screen, and `--help`
lists the options.

The meters and the log directory are remembered in
`~/.local/state/bm78x-tui/saved` (or under `$XDG_STATE_HOME`). The meter's
clock is set to the host's local time on each connect.

[TESTING.md](https://github.com/kevstewa/bm78x-tui/blob/main/TESTING.md)
says what the tests check and what has been confirmed on a real meter.

Licensed under the MIT license.
