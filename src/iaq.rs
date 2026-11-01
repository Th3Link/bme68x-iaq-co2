//! The air quality from the gas resistance: humidity compensation, the
//! calibration stages and the gas ceiling.

use crate::baseline::Baseline;
use crate::compensation::{self, Climate};
use crate::donchian::Donchian;
use crate::model::{Equivalents, Model};

/// One measurement. Take them at a constant cycle (see [`CYCLE_MS`]): the
/// gas resistance depends on how long the heater rested before.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// When it was taken, in ms on any monotonic clock.
    pub time_ms: u64,
    /// °C, as the sensor reads it.
    pub temperature: f32,
    /// % RH, as the sensor reads it.
    pub humidity: f32,
    /// Gas resistance in Ω.
    pub gas_ohm: f32,
    /// Whether the gas measurement is valid and the heater reached its
    /// temperature. Without, the sample only updates the climate.
    pub gas_valid: bool,
}

/// The settings; [`Config::default`] holds `SE_BME680`'s defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Added to the temperature (°C) for the [`Climate`], usually negative
    /// for the sensor's self-heating. The air quality uses the sensor's own
    /// readings.
    pub temperature_offset: f32,
    /// Slope of the humidity compensation per g/m³ (0.01–0.1).
    pub humidity_slope: f32,
    /// A compensated gas resistance below this (Ω, before compensation)
    /// never counts as clean air: against calibrating to bad air at the
    /// start.
    pub gas_min_ohm: f32,
    /// A gas resistance above this (Ω) is taken as a spurious reading and
    /// skipped.
    pub gas_max_ohm: f32,
    /// Shortest time before the sensor counts as warmed up.
    pub init_ms: u32,
    /// Shortest burn-in, collecting the first gas ceiling.
    pub burn_in_ms: u32,
    /// In operation, one value of the ceiling is replaced after this time
    /// without a new high, so that the ceiling follows drift.
    pub decay_ms: u32,
    /// Range limits of the smoothing (temperature, humidity, gas), 0 for
    /// none; only with smoothing (`W` > 0).
    pub smoothing_limits: [f32; 3],
    /// Index and equivalents.
    pub model: Model,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            temperature_offset: 0.0,
            humidity_slope: 0.03,
            gas_min_ohm: 50_000.0,
            gas_max_ohm: 225_000.0,
            init_ms: 30_000,
            burn_in_ms: 5 * 60_000,
            decay_ms: 30 * 60_000,
            smoothing_limits: [0.0; 3],
            model: Model::default(),
        }
    }
}

/// Recommended heater temperature, °C.
pub const HEATER_TEMPERATURE_C: u16 = 320;
/// Recommended heater duration, ms.
pub const HEATER_DURATION_MS: u16 = 150;
/// Recommended measurement cycle, ms.
pub const CYCLE_MS: u32 = 3_000;

/// Where the calibration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for the heated sensor to settle (its resistance stops
    /// falling).
    Init,
    /// Collecting the first gas ceiling.
    BurnIn,
    /// Calibrated; the ceiling follows new highs and drift.
    Operation,
}

/// How far to trust the air quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Accuracy {
    /// Not calibrated; the values mean nothing.
    Unreliable = 0,
    /// Burn-in, or a wide ceiling.
    Low = 1,
    /// The ceiling's values lie within 8 %.
    Medium = 2,
    /// Within 3.5 %, after at least two decay periods.
    High = 3,
    /// Within 2 %, after at least 100 decay periods (days).
    VeryHigh = 4,
}

/// The air quality and what follows from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Air {
    /// 0–100 %, 100 = as clean as the gas ceiling.
    pub quality: f32,
    /// Index 0 (clean) to 500, breath-VOC and CO2 equivalents.
    pub equivalents: Equivalents,
    /// How far to trust it.
    pub accuracy: Accuracy,
    /// Calibration stage.
    pub stage: Stage,
    /// The last humidity-compensated gas resistance, Ω (0 before the first).
    pub compensated_gas_ohm: f32,
    /// The gas ceiling, Ω (0 before the burn-in).
    pub ceiling_ohm: f32,
}

/// What a sample gives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// Temperature and humidity with the offset applied.
    pub climate: Climate,
    /// The air quality (unchanged by a sample without valid gas).
    pub air: Air,
}

/// The air quality calculation for one sensor. `N` values make the gas
/// ceiling; `W` > 0 smooths temperature, humidity and gas over the last `W`
/// samples (Donchian) after the warm-up.
#[derive(Debug, Clone)]
pub struct AirQuality<const N: usize = 100, const W: usize = 0> {
    pub(crate) config: Config,
    pub(crate) baseline: Baseline<N>,
    pub(crate) stage: Stage,
    /// After a restore: the stage to go back to once warmed up, and how far
    /// into its period it was.
    pub(crate) resume: Option<(Stage, u64)>,
    /// Start of the current period (`SE_BME680`'s calibration timer).
    pub(crate) timer_ms: u64,
    pub(crate) last_ms: Option<u64>,
    /// Warm-up: the lowest raw resistance yet, and readings above it since.
    lowest: f32,
    higher_lows: u8,
    /// Decay periods in operation.
    pub(crate) uptime: u32,
    quality: f32,
    compensated: f32,
    smoothing: [Donchian<W>; 3],
}

impl<const N: usize, const W: usize> AirQuality<N, W> {
    /// A calculation that starts from nothing.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let [t, h, g] = config.smoothing_limits;
        Self {
            config,
            baseline: Baseline::new(),
            stage: Stage::Init,
            resume: None,
            timer_ms: 0,
            last_ms: None,
            lowest: 0.0,
            higher_lows: 0,
            uptime: 0,
            // Neutral until there is a ceiling.
            quality: 50.0,
            compensated: 0.0,
            smoothing: [Donchian::new(t), Donchian::new(h), Donchian::new(g)],
        }
    }

    /// The settings.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Takes a sample in.
    pub fn update(&mut self, sample: &Sample) -> Output {
        let climate = compensation::climate(
            sample.temperature,
            sample.humidity,
            self.config.temperature_offset,
        );
        let elapsed = if let Some(last) = self.last_ms {
            sample.time_ms.saturating_sub(last)
        } else {
            self.timer_ms = sample.time_ms;
            0
        };
        self.last_ms = Some(sample.time_ms);
        if sample.gas_valid && sample.gas_ohm.is_finite() && sample.gas_ohm > 0.0 {
            self.step(sample, elapsed);
        }
        Output {
            climate,
            air: self.air(),
        }
    }

    /// The air quality as of the last sample.
    #[must_use]
    pub fn air(&self) -> Air {
        Air {
            quality: self.quality,
            equivalents: self.config.model.equivalents(self.quality),
            accuracy: self.accuracy(),
            stage: self.stage,
            compensated_gas_ohm: self.compensated,
            ceiling_ohm: self.baseline.ceiling(),
        }
    }

    fn step(&mut self, sample: &Sample, elapsed: u64) {
        let now = sample.time_ms;
        if sample.gas_ohm > self.config.gas_max_ohm {
            // Spurious; before operation, don't count its time either.
            if self.stage != Stage::Operation {
                self.timer_ms += elapsed;
            }
            return;
        }
        let (temperature, humidity, gas) = if W > 0 && self.stage != Stage::Init {
            // Not during the warm-up: its falling readings would widen the
            // channels.
            let [t, h, g] = &mut self.smoothing;
            (
                t.track(sample.temperature),
                h.track(sample.humidity),
                g.track(sample.gas_ohm),
            )
        } else {
            (sample.temperature, sample.humidity, sample.gas_ohm)
        };
        let factor = compensation::gas_factor(temperature, humidity, self.config.humidity_slope);
        let compensated = gas * factor;
        let compensated_min = self.config.gas_min_ohm * factor;
        if !compensated.is_finite() || !compensated_min.is_finite() {
            return;
        }
        let period = now.saturating_sub(self.timer_ms);
        match self.stage {
            Stage::Init => {
                if period >= u64::from(self.config.init_ms) {
                    self.warm_up(sample.gas_ohm, now);
                }
            }
            Stage::BurnIn => {
                if period < u64::from(self.config.burn_in_ms) || !self.baseline.full() {
                    self.baseline.add(compensated.max(compensated_min), true);
                } else {
                    self.timer_ms = now;
                    self.stage = Stage::Operation;
                }
            }
            Stage::Operation => {
                if compensated > compensated_min {
                    if compensated > self.baseline.ceiling() {
                        self.baseline.add(compensated, true);
                    } else if period >= u64::from(self.config.decay_ms) {
                        self.baseline.add(compensated, false);
                        self.timer_ms = now;
                        self.uptime = self.uptime.saturating_add(1);
                    }
                }
            }
        }
        self.compensated = compensated;
        if !self.baseline.is_empty() {
            let ratio = compensated / self.baseline.ceiling();
            self.quality = (ratio * ratio * 100.0).min(100.0);
        }
    }

    /// The heated sensor's resistance falls at first; once it read above its
    /// lowest value three times, it has settled.
    fn warm_up(&mut self, gas: f32, now: u64) {
        if self.lowest == 0.0 || gas < self.lowest {
            self.lowest = gas;
            self.higher_lows = 0;
        } else if gas > self.lowest {
            self.higher_lows += 1;
            if self.higher_lows >= 3 {
                if let Some((stage, into_period)) = self.resume.take() {
                    self.stage = stage;
                    self.timer_ms = now.saturating_sub(into_period);
                } else {
                    self.stage = Stage::BurnIn;
                    self.timer_ms = now;
                }
            }
        }
    }

    /// How far to trust the air quality now.
    #[must_use]
    pub fn accuracy(&self) -> Accuracy {
        match self.stage {
            Stage::Init => Accuracy::Unreliable,
            Stage::BurnIn => Accuracy::Low,
            Stage::Operation => {
                let range = self.baseline.range();
                if range < 0.020 && self.uptime >= 100 {
                    Accuracy::VeryHigh
                } else if range < 0.035 && self.uptime >= 2 {
                    Accuracy::High
                } else if range < 0.080 {
                    Accuracy::Medium
                } else {
                    Accuracy::Low
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample every 3 s at 25 °C / 0 % RH (no humidity compensation).
    fn dry(time_s: u64, gas: f32) -> Sample {
        Sample {
            time_ms: time_s * 1000,
            temperature: 25.0,
            humidity: 0.0,
            gas_ohm: gas,
            gas_valid: true,
        }
    }

    fn config() -> Config {
        Config {
            gas_min_ohm: 1_000.0,
            ..Config::default()
        }
    }

    /// Runs the warm-up: falling, then three readings above the low.
    fn warmed_up<const N: usize>(iaq: &mut AirQuality<N>) -> u64 {
        let mut t = 0;
        for gas in [90_000.0, 80_000.0, 70_000.0, 60_000.0, 50_000.0] {
            iaq.update(&dry(t, gas));
            t += 10;
        }
        assert_eq!(iaq.stage, Stage::Init);
        for gas in [51_000.0, 52_000.0, 53_000.0] {
            iaq.update(&dry(t, gas));
            t += 10;
        }
        assert_eq!(iaq.stage, Stage::BurnIn);
        t
    }

    #[test]
    fn starts_unreliable_and_neutral() {
        let mut iaq = AirQuality::<4>::new(config());
        let out = iaq.update(&dry(0, 100_000.0));
        assert_eq!(out.air.accuracy, Accuracy::Unreliable);
        assert_eq!(out.air.quality, 50.0);
        assert_eq!(out.air.ceiling_ohm, 0.0);
    }

    #[test]
    fn warm_up_waits_for_the_minimum_time() {
        let mut iaq = AirQuality::<4>::new(config());
        // Rising right away, but within the first 30 s.
        for (i, gas) in [50_000.0, 60_000.0, 70_000.0, 80_000.0].iter().enumerate() {
            iaq.update(&dry(i as u64 * 3, *gas));
        }
        assert_eq!(iaq.stage, Stage::Init);
    }

    #[test]
    fn a_new_low_restarts_the_count() {
        let mut iaq = AirQuality::<4>::new(config());
        // The warm-up's minimum time counts from the first sample.
        iaq.update(&dry(0, 60_000.0));
        let mut t = 30;
        for gas in [50_000.0, 51_000.0, 52_000.0, 40_000.0, 41_000.0, 42_000.0] {
            iaq.update(&dry(t, gas));
            t += 3;
        }
        assert_eq!(iaq.stage, Stage::Init);
        iaq.update(&dry(t, 43_000.0));
        assert_eq!(iaq.stage, Stage::BurnIn);
    }

    #[test]
    fn burn_in_needs_time_and_a_full_ceiling() {
        let mut iaq = AirQuality::<4>::new(config());
        let mut t = warmed_up(&mut iaq);
        let start = iaq.timer_ms / 1000;
        // Full after four samples, but the 5 minutes aren't over.
        while t < start + 299 {
            iaq.update(&dry(t, 100_000.0));
            assert_eq!(iaq.stage, Stage::BurnIn);
            assert_eq!(iaq.accuracy(), Accuracy::Low);
            t += 3;
        }
        iaq.update(&dry(start + 300, 100_000.0));
        assert_eq!(iaq.stage, Stage::Operation);
        assert_eq!(iaq.air().ceiling_ohm, 100_000.0);
        // All four equal: range 0, but no decay period yet.
        assert_eq!(iaq.accuracy(), Accuracy::Medium);
    }

    #[test]
    fn quality_is_the_squared_ratio_to_the_ceiling() {
        let mut iaq = AirQuality::<4>::new(config());
        let mut t = warmed_up(&mut iaq);
        for _ in 0..4 {
            iaq.update(&dry(t, 100_000.0));
            t += 3;
        }
        let out = iaq.update(&dry(t, 50_000.0));
        assert!((out.air.quality - 25.0).abs() < 1e-3);
        // The burn-in collects the higher of the reading and the minimum.
        let out = iaq.update(&dry(t + 3, 120_000.0));
        assert_eq!(out.air.quality, 100.0);
    }

    #[test]
    fn operation_takes_new_highs_and_rotates_on_decay() {
        let mut iaq = AirQuality::<4>::new(config());
        let mut t = warmed_up(&mut iaq);
        let start = t;
        while t <= start + 300 {
            iaq.update(&dry(t, 100_000.0));
            t += 3;
        }
        assert_eq!(iaq.stage, Stage::Operation);
        // A new high goes in right away.
        iaq.update(&dry(t, 140_000.0));
        assert_eq!(iaq.air().ceiling_ohm, 110_000.0);
        t += 3;
        // Lower readings change nothing until the decay period is over.
        let period_start = iaq.timer_ms / 1000;
        while t < period_start + 1800 {
            iaq.update(&dry(t, 90_000.0));
            t += 3;
        }
        assert_eq!(iaq.air().ceiling_ohm, 110_000.0);
        assert_eq!(iaq.uptime, 0);
        iaq.update(&dry(t, 90_000.0));
        assert_eq!(iaq.uptime, 1);
        assert!(iaq.air().ceiling_ohm < 110_000.0);
    }

    #[test]
    fn accuracy_follows_range_and_uptime() {
        let mut iaq = AirQuality::<4>::new(config());
        iaq.stage = Stage::Operation;
        iaq.baseline = Baseline::from_parts([100.0, 100.0, 100.0, 90.0], 0);
        assert_eq!(iaq.accuracy(), Accuracy::Low);
        iaq.baseline = Baseline::from_parts([100.0, 100.0, 100.0, 95.0], 0);
        assert_eq!(iaq.accuracy(), Accuracy::Medium);
        iaq.baseline = Baseline::from_parts([100.0, 100.0, 100.0, 97.0], 0);
        assert_eq!(iaq.accuracy(), Accuracy::Medium);
        iaq.uptime = 2;
        assert_eq!(iaq.accuracy(), Accuracy::High);
        iaq.baseline = Baseline::from_parts([100.0, 100.0, 100.0, 99.0], 0);
        assert_eq!(iaq.accuracy(), Accuracy::High);
        iaq.uptime = 100;
        assert_eq!(iaq.accuracy(), Accuracy::VeryHigh);
    }

    #[test]
    fn bad_air_at_the_start_is_not_clean_air() {
        // Started in bad air: everything reads 20 kΩ. The burn-in collects
        // the minimum (50 kΩ) instead, so the quality shows it.
        let mut iaq = AirQuality::<4>::new(Config::default());
        iaq.update(&dry(0, 30_000.0));
        let mut t = 30;
        for gas in [20_000.0, 19_000.0, 19_500.0, 19_600.0, 19_700.0] {
            iaq.update(&dry(t, gas));
            t += 3;
        }
        assert_eq!(iaq.stage, Stage::BurnIn);
        let out = iaq.update(&dry(t, 20_000.0));
        assert_eq!(out.air.ceiling_ohm, 50_000.0);
        assert!((out.air.quality - 16.0).abs() < 1e-3);
    }

    #[test]
    fn spurious_and_invalid_readings_are_skipped() {
        let mut iaq = AirQuality::<4>::new(config());
        let t = warmed_up(&mut iaq);
        iaq.update(&dry(t, 100_000.0));
        let before = iaq.air();
        let timer = iaq.timer_ms;
        // Too high: skipped, and its time doesn't count towards the burn-in.
        iaq.update(&dry(t + 3, 500_000.0));
        assert_eq!(iaq.air(), before);
        assert_eq!(iaq.timer_ms, timer + 3000);
        // Invalid or nonsense: only the climate.
        for (gas, valid) in [
            (80_000.0, false),
            (f32::NAN, true),
            (0.0, true),
            (-1.0, true),
        ] {
            let out = iaq.update(&Sample {
                gas_ohm: gas,
                gas_valid: valid,
                ..dry(t + 6, 0.0)
            });
            assert_eq!(out.air, before);
        }
    }

    #[test]
    fn humidity_raises_the_compensated_resistance() {
        let mut iaq = AirQuality::<4>::new(config());
        let t = warmed_up(&mut iaq);
        let humid = iaq.update(&Sample {
            humidity: 50.0,
            ..dry(t, 100_000.0)
        });
        let expected = 100_000.0 * compensation::gas_factor(25.0, 50.0, 0.03);
        assert!((humid.air.compensated_gas_ohm - expected).abs() < 1.0);
    }

    #[test]
    fn smoothing_starts_after_the_warm_up() {
        let mut iaq = AirQuality::<4, 3>::new(config());
        let mut t = 0;
        for gas in [
            90_000.0, 80_000.0, 70_000.0, 60_000.0, 50_000.0, 51_000.0, 52_000.0, 53_000.0,
        ] {
            iaq.update(&dry(t, gas));
            t += 10;
        }
        assert_eq!(iaq.stage, Stage::BurnIn);
        iaq.update(&dry(t, 100_000.0));
        let out = iaq.update(&dry(t + 3, 60_000.0));
        // Middle of 100 k and 60 k, not 60 k.
        assert_eq!(out.air.compensated_gas_ohm, 80_000.0);
    }

    #[test]
    fn climate_has_the_offset() {
        let mut iaq = AirQuality::<4>::new(Config {
            temperature_offset: -3.0,
            ..config()
        });
        let out = iaq.update(&Sample {
            humidity: 40.0,
            ..dry(0, 100_000.0)
        });
        assert_eq!(out.climate.temperature, 22.0);
        assert!(out.climate.humidity > 40.0);
    }
}
