//! Donchian channel smoothing: the middle between the minimum and the maximum
//! of the last `W` values. It takes out slow oscillations, e.g. from a
//! heater or an air conditioner cycling, without lagging like a mean.

/// The last `W` values and the middle of their range.
#[derive(Debug, Clone)]
pub struct Donchian<const W: usize> {
    data: [f32; W],
    next: usize,
    full: bool,
    /// The largest range kept, 0 for no limit. A larger one shortens the
    /// look-back, to the side the newest value broke out to.
    limit: f32,
}

impl<const W: usize> Donchian<W> {
    /// An empty channel; `limit` 0 for no range limit.
    #[must_use]
    pub const fn new(limit: f32) -> Self {
        Self {
            data: [0.0; W],
            next: 0,
            full: false,
            limit,
        }
    }

    /// Takes `value` in and returns the middle of the channel. With `W` 0
    /// it returns `value`.
    pub fn track(&mut self, value: f32) -> f32 {
        if W == 0 {
            return value;
        }
        self.data[self.next] = value;
        self.next += 1;
        if self.next == W {
            self.next = 0;
            self.full = true;
        }
        let count = if self.full { W } else { self.next };
        let (mut min, mut max) = (value, value);
        // Newest first, so that a range limit cuts off the oldest values.
        let mut k = (self.next + W - 1) % W;
        for _ in 1..count {
            k = (k + W - 1) % W;
            let d = self.data[k];
            min = min.min(d);
            max = max.max(d);
            if self.limit != 0.0 && max - min > self.limit {
                if max - value < value - min {
                    // Broke out upwards.
                    min = max - self.limit;
                } else {
                    max = min + self.limit;
                }
                break;
            }
        }
        min.midpoint(max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_of_the_range() {
        let mut d = Donchian::<4>::new(0.0);
        assert_eq!(d.track(10.0), 10.0);
        assert_eq!(d.track(20.0), 15.0);
        assert_eq!(d.track(12.0), 15.0);
        assert_eq!(d.track(14.0), 15.0);
        // 10 drops out of the window: 20, 12, 14, 16.
        assert_eq!(d.track(16.0), 16.0);
        // 20 drops out: 12, 14, 16, 13.
        assert_eq!(d.track(13.0), 14.0);
    }

    #[test]
    fn zero_width_passes_through() {
        let mut d = Donchian::<0>::new(0.0);
        assert_eq!(d.track(3.5), 3.5);
    }

    #[test]
    fn range_limit_follows_a_breakout() {
        let mut d = Donchian::<8>::new(4.0);
        for _ in 0..8 {
            d.track(10.0);
        }
        // Up by 10: the range is held at 4 below the new maximum.
        assert_eq!(d.track(20.0), 18.0);
        // Down: held at 4 above the minimum.
        let mut d = Donchian::<8>::new(4.0);
        for _ in 0..8 {
            d.track(10.0);
        }
        assert_eq!(d.track(0.0), 2.0);
    }
}
