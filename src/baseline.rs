//! The gas "ceiling": the mean of the highest humidity-compensated gas
//! resistances seen, which stands for clean air.

/// `N` compensated gas resistances (Ω, 0 = empty) and what follows from them.
#[derive(Debug, Clone)]
pub(crate) struct Baseline<const N: usize> {
    pub(crate) data: [f32; N],
    /// Where the next value goes when it isn't replacing the smallest.
    pub(crate) next: usize,
    /// Mean of the non-empty values; 0 while there are none.
    ceiling: f32,
    /// `(max − min) / max` of the non-empty values; 1 while there are none.
    range: f32,
}

impl<const N: usize> Baseline<N> {
    const NOT_EMPTY: () = assert!(N > 0, "the baseline needs at least one value");

    pub(crate) const fn new() -> Self {
        let () = Self::NOT_EMPTY;
        Self {
            data: [0.0; N],
            next: 0,
            ceiling: 0.0,
            range: 1.0,
        }
    }

    /// From stored values, see [`crate::AirQuality::restore`].
    pub(crate) fn from_parts(data: [f32; N], next: usize) -> Self {
        let mut baseline = Self {
            data,
            next: next % N,
            ceiling: 0.0,
            range: 1.0,
        };
        baseline.recompute();
        baseline
    }

    pub(crate) fn ceiling(&self) -> f32 {
        self.ceiling
    }

    pub(crate) fn range(&self) -> f32 {
        self.range
    }

    /// Whether every place has a value (they fill in order).
    pub(crate) fn full(&self) -> bool {
        self.data[N - 1] > 0.0
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ceiling == 0.0
    }

    /// Takes `value` in. Once full and with `replace_smallest`, it replaces
    /// the smallest value if it is larger; otherwise it goes into the next
    /// place in turn (which rotates the oldest out once full).
    pub(crate) fn add(&mut self, value: f32, replace_smallest: bool) {
        if replace_smallest && self.full() {
            let (index, smallest) =
                self.data
                    .iter()
                    .copied()
                    .enumerate()
                    .fold(
                        (0, f32::INFINITY),
                        |best, (i, v)| {
                            if v < best.1 {
                                (i, v)
                            } else {
                                best
                            }
                        },
                    );
            if value > smallest {
                self.data[index] = value;
            }
        } else {
            self.data[self.next] = value;
            self.next = (self.next + 1) % N;
        }
        self.recompute();
    }

    fn recompute(&mut self) {
        let (mut sum, mut min, mut max, mut count) = (0.0f32, f32::INFINITY, 0.0f32, 0u32);
        for &v in self.data.iter().filter(|v| **v > 0.0) {
            sum += v;
            min = min.min(v);
            max = max.max(v);
            count += 1;
        }
        if count > 0 {
            self.range = (max - min) / max;
            let mean = sum / count as f32;
            if mean.is_finite() {
                self.ceiling = mean;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_order_then_replaces_the_smallest() {
        let mut b = Baseline::<3>::new();
        assert!(b.is_empty());
        assert_eq!(b.range(), 1.0);
        b.add(100.0, true);
        assert_eq!(b.ceiling(), 100.0);
        assert_eq!(b.range(), 0.0);
        b.add(200.0, true);
        assert!(!b.full());
        b.add(300.0, true);
        assert!(b.full());
        assert_eq!(b.ceiling(), 200.0);
        // Smaller than everything: nothing changes.
        b.add(50.0, true);
        assert_eq!(b.data, [100.0, 200.0, 300.0]);
        // Larger than the smallest: replaces it.
        b.add(400.0, true);
        assert_eq!(b.data, [400.0, 200.0, 300.0]);
        assert_eq!(b.ceiling(), 300.0);
        assert_eq!(b.range(), 0.5);
    }

    #[test]
    fn rotation_replaces_in_turn() {
        let mut b = Baseline::<3>::new();
        for v in [100.0, 200.0, 300.0] {
            b.add(v, true);
        }
        b.add(10.0, false);
        assert_eq!(b.data, [10.0, 200.0, 300.0]);
        b.add(20.0, false);
        assert_eq!(b.data, [10.0, 20.0, 300.0]);
    }

    #[test]
    fn from_parts_recomputes() {
        let b = Baseline::<4>::from_parts([100.0, 300.0, 0.0, 0.0], 6);
        assert_eq!(b.ceiling(), 200.0);
        assert_eq!(b.next, 2);
        assert!(!b.full());
    }
}
