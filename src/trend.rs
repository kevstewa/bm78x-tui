//! The history behind the trend plot.

use std::collections::VecDeque;

/// The selectable plot windows, in seconds.
pub const WINDOWS: [u32; 5] = [10, 30, 60, 300, 600];

/// Readings in base units against time in seconds, for the longest window.
#[derive(Debug, Clone, Default)]
pub struct Trend {
    points: VecDeque<(f64, f64)>,
}

impl Trend {
    /// Adds a reading taken at `time` seconds, and drops readings older than
    /// the longest window.
    pub fn push(&mut self, time: f64, value: f64) {
        let oldest = time - f64::from(WINDOWS[WINDOWS.len() - 1]);
        while self.points.front().is_some_and(|&(t, _)| t < oldest) {
            self.points.pop_front();
        }
        self.points.push_back((time, value));
    }

    /// Forgets every reading.
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// The readings of the last `span` seconds before `now`, with time
    /// relative to `now` (so from `-span` to 0).
    pub fn window(&self, now: f64, span: f64) -> Vec<(f64, f64)> {
        self.points
            .iter()
            .filter(|&&(t, _)| t >= now - span && t <= now)
            .map(|&(t, v)| (t - now, v))
            .collect()
    }
}

/// Y-axis bounds for `points`, padded so the line does not sit on the edge.
pub fn y_bounds(points: &[(f64, f64)]) -> [f64; 2] {
    let Some(&(_, first)) = points.first() else {
        return [0.0, 1.0];
    };
    let (low, high) = points
        .iter()
        .fold((first, first), |(lo, hi), &(_, v)| (lo.min(v), hi.max(v)));
    let pad = if high > low {
        (high - low) * 0.05
    } else if low != 0.0 {
        // A flat line: show it mid-height.
        low.abs() * 0.01
    } else {
        1.0
    };
    [low - pad, high + pad]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_relative_to_now() {
        let mut trend = Trend::default();
        for t in 0..=10 {
            trend.push(f64::from(t), f64::from(t) * 2.0);
        }
        assert_eq!(
            trend.window(10.0, 3.0),
            vec![(-3.0, 14.0), (-2.0, 16.0), (-1.0, 18.0), (0.0, 20.0)]
        );
        // Nothing recent: an empty plot, not an old one.
        assert!(trend.window(100.0, 10.0).is_empty());
    }

    #[test]
    fn keeps_only_the_longest_window() {
        let mut trend = Trend::default();
        trend.push(0.0, 1.0);
        trend.push(600.0, 2.0);
        trend.push(601.0, 3.0);
        assert_eq!(trend.window(601.0, 10_000.0), vec![(-1.0, 2.0), (0.0, 3.0)]);
    }

    #[test]
    fn clear_forgets() {
        let mut trend = Trend::default();
        trend.push(1.0, 1.0);
        trend.clear();
        assert!(trend.window(1.0, 10.0).is_empty());
    }

    #[test]
    fn bounds_pad_the_data() {
        let [low, high] = y_bounds(&[(0.0, 1.0), (1.0, 3.0)]);
        assert!(low < 1.0 && low > 0.8, "{low}");
        assert!(high > 3.0 && high < 3.2, "{high}");
    }

    #[test]
    fn bounds_of_a_flat_line_are_not_empty() {
        for v in [0.0, 3.3, -0.001] {
            let [low, high] = y_bounds(&[(0.0, v), (1.0, v)]);
            assert!(low < v && v < high, "{v}: {low}..{high}");
        }
    }

    #[test]
    fn bounds_without_data() {
        let [low, high] = y_bounds(&[]);
        assert!(low < high);
    }
}
