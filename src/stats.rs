//! Running MIN, MAX and AVG.

/// Running statistics of readings in base units.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    min: f64,
    max: f64,
    sum: f64,
    count: u64,
}

impl Stats {
    /// Adds a reading.
    pub fn push(&mut self, value: f64) {
        if self.count == 0 {
            (self.min, self.max) = (value, value);
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
        self.sum += value;
        self.count += 1;
    }

    /// Forgets every reading.
    pub fn clear(&mut self) {
        *self = Stats::default();
    }

    /// The smallest reading.
    pub fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    /// The largest reading.
    pub fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    /// The mean of the readings.
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }

    /// How many readings there are.
    pub fn count(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let stats = Stats::default();
        assert_eq!(
            (stats.min(), stats.max(), stats.mean(), stats.count()),
            (None, None, None, 0)
        );
    }

    #[test]
    fn running_values() {
        let mut stats = Stats::default();
        for v in [3.0, -1.0, 4.0] {
            stats.push(v);
        }
        assert_eq!(stats.min(), Some(-1.0));
        assert_eq!(stats.max(), Some(4.0));
        assert_eq!(stats.mean(), Some(2.0));
        assert_eq!(stats.count(), 3);
    }

    #[test]
    fn one_value_is_its_own_min_and_max() {
        let mut stats = Stats::default();
        stats.push(-7.5);
        assert_eq!(
            (stats.min(), stats.max(), stats.mean()),
            (Some(-7.5), Some(-7.5), Some(-7.5))
        );
    }

    #[test]
    fn clear_forgets() {
        let mut stats = Stats::default();
        stats.push(1.0);
        stats.clear();
        assert_eq!(stats.count(), 0);
        assert_eq!(stats.min(), None);
        stats.push(5.0);
        assert_eq!(stats.min(), Some(5.0));
    }
}
