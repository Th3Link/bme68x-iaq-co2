//! From the air quality to an index on Bosch's 0–500 scale, a breath-VOC
//! equivalent and a CO2 equivalent.
//!
//! A metal-oxide sensor's resistance follows a power law in the reducing gas
//! concentration: `R / R0 = (C / C0)^−β`. With the air quality
//! `q = 100 % × (R / R0)²` that gives the concentration relative to clean
//! air, `n = C / C0 = (100 % / q)^(1 / 2β)`. The index is linear in `log n`
//! (a decade of VOCs is the same step anywhere on the scale); the breath-VOC
//! equivalent is `n` times the clean-air value; the CO2 equivalent is
//! proportional to the index, as Bosch describes for BSEC.
//!
//! # Where the default numbers come from
//!
//! Only published anchor points, no fit (see the README):
//! - Bosch: an IAQ of 25 is "typical good air", 250 "typical polluted air";
//!   0–50 is "excellent".
//! - BSEC reports clean, uncalibrated air as IAQ 50, a CO2 equivalent of
//!   500 ppm and a breath-VOC equivalent of 0.5 ppm, and (Bosch community
//!   forum) its CO2 equivalent is about ten times the static IAQ.
//! - β = 0.5 lies within the range given for tin-oxide sensors and makes the
//!   air quality's 10 % "typical polluted" (ten times the clean-air VOCs).
//!
//! These are estimates from VOCs. The CO2 equivalent is not a CO2 measurement.

use libm::{log10f, powf};

/// The constants of the model; [`Model::default`] holds the anchors above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    /// β of `R / R0 = (C / C0)^−β`.
    pub mox_exponent: f32,
    /// Index of clean air (the gas ceiling).
    pub clean_index: f32,
    /// Index of "typical polluted" air.
    pub polluted_index: f32,
    /// VOC concentration of "typical polluted" air relative to clean air.
    pub polluted_ratio: f32,
    /// Breath-VOC equivalent of clean air, in ppm.
    pub clean_bvoc_ppm: f32,
    /// CO2 equivalent per index point, in ppm.
    pub eco2_per_index: f32,
    /// The CO2 equivalent never goes below this (outdoor air), in ppm.
    pub eco2_floor_ppm: f32,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            mox_exponent: 0.5,
            clean_index: 25.0,
            polluted_index: 250.0,
            polluted_ratio: 10.0,
            clean_bvoc_ppm: 0.5,
            eco2_per_index: 10.0,
            eco2_floor_ppm: 400.0,
        }
    }
}

/// The highest index.
pub const MAX_INDEX: f32 = 500.0;

/// What the model makes of one air quality value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Equivalents {
    /// 0 (clean) to 500 (heavily polluted).
    pub index: f32,
    /// Breath-VOC equivalent, ppm.
    pub bvoc_ppm: f32,
    /// CO2 equivalent, ppm.
    pub eco2_ppm: f32,
}

impl Model {
    /// The index and the equivalents for an air quality of `quality` %
    /// (0–100, 100 = the gas ceiling).
    #[must_use]
    pub fn equivalents(&self, quality: f32) -> Equivalents {
        // log10 n = log10(100 / q) / 2β; 0 % is as bad as the scale goes.
        let log_ratio = if quality > 0.0 {
            log10f(100.0 / quality.min(100.0)) / (2.0 * self.mox_exponent)
        } else {
            f32::INFINITY
        };
        let per_decade = (self.polluted_index - self.clean_index) / log10f(self.polluted_ratio);
        let index = (self.clean_index + per_decade * log_ratio).clamp(0.0, MAX_INDEX);
        // From the clamped index, so that all three stay consistent.
        let ratio = powf(10.0, (index - self.clean_index) / per_decade);
        Equivalents {
            index,
            bvoc_ppm: self.clean_bvoc_ppm * ratio,
            eco2_ppm: (self.eco2_per_index * index).max(self.eco2_floor_ppm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn anchors() {
        let m = Model::default();
        let clean = m.equivalents(100.0);
        assert!(close(clean.index, 25.0, 1e-4));
        assert!(close(clean.bvoc_ppm, 0.5, 1e-5));
        assert_eq!(clean.eco2_ppm, 400.0);
        // Ten times the clean-air VOCs: 10 % air quality with β = 0.5.
        let polluted = m.equivalents(10.0);
        assert!(close(polluted.index, 250.0, 1e-3));
        assert!(close(polluted.bvoc_ppm, 5.0, 1e-4));
        assert!(close(polluted.eco2_ppm, 2500.0, 0.01));
        // BSEC's "unknown" point: IAQ 50 is 500 ppm.
        let q = 100.0 / powf(10.0, 2.0 * 0.5 * (25.0 / 225.0));
        let e = m.equivalents(q);
        assert!(close(e.index, 50.0, 1e-3));
        assert!(close(e.eco2_ppm, 500.0, 0.05));
    }

    #[test]
    fn monotonic_and_clamped() {
        let m = Model::default();
        let mut last = m.equivalents(100.0);
        for step in (0..100).rev() {
            let e = m.equivalents(step as f32);
            assert!(e.index >= last.index, "index at {step} %");
            assert!(e.bvoc_ppm >= last.bvoc_ppm);
            assert!(e.eco2_ppm >= last.eco2_ppm);
            last = e;
        }
        let worst = m.equivalents(0.0);
        assert_eq!(worst.index, MAX_INDEX);
        assert!(worst.bvoc_ppm.is_finite());
        assert_eq!(worst.eco2_ppm, 5000.0);
        // Above 100 % counts as 100 %.
        assert_eq!(m.equivalents(140.0), m.equivalents(100.0));
    }

    #[test]
    fn no_nan() {
        let m = Model::default();
        for q in [f32::MIN_POSITIVE, 1e-30, 0.001, -5.0, f32::NAN] {
            let e = m.equivalents(q);
            assert!(e.index.is_finite(), "{q}");
            assert!(e.bvoc_ppm.is_finite(), "{q}");
            assert!(e.eco2_ppm.is_finite(), "{q}");
        }
    }
}
