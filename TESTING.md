# Testing

## Without a meter

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The unit tests in `src/` cover the TUI's logic, its files, and its screens
rendered to a test terminal. `tests/captures/` holds frames captured from a
BM787BT, with what its LCD showed, to check the displayed text against. The
decoding and the connection are tested in the
[bm78x](https://github.com/kevstewa/bm78x) library.

## Confirmed on a BM787BT

On 2026-10-23: picking the meter from the menu, connecting by `--address`
when BlueZ has not seen it yet, the display, trend and statistics, CSV
logging (5 rows a second, including while paused), the raw view, and
quitting, which releases the meter.

On 2026-10-24:

- With the menu open and scanning, a lost link reconnects.
- SIGTERM and SIGHUP (closing the terminal) release the meter: BlueZ shows
  it disconnected, and a new connect succeeds straight after. Quitting takes
  about 2.9 s, BlueZ's disconnect.

On 2026-10-25, a Sunday:

- Connecting sets the meter's clock to the host's local time, to the
  second. Before the bm78x fix for weekdays, the meter refused the time on
  Sundays and the connect failed.
- With `--address` for a meter that is not there, the status line says it
  was not found and how to turn its Bluetooth on, after BlueZ's 10 s
  discovery.
- Quitting releases the meter.
