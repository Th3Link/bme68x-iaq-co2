//! An open indoor air quality calculation for the Bosch BME680 and BME688,
//! without Bosch's closed BSEC library.
//!
//! From temperature, humidity and the gas resistance of each measurement it
//! gives:
//! - an **air quality** of 0–100 % (100 = as clean as the cleanest air the
//!   sensor saw), from the humidity-compensated gas resistance against a
//!   self-calibrating "gas ceiling";
//! - an **index** of 0–500 (0 = clean) like Bosch's IAQ scale, a
//!   **breath-VOC equivalent** and a **CO2 equivalent** in ppm, from a
//!   documented model ([`Model`]);
//! - an **accuracy** of the calibration, and the climate with the sensor's
//!   self-heating taken out.
//!
//! The algorithm is a Rust rewrite of [SE_BME680] (MIT, Steve Eidemiller),
//! itself based on [raspi-bme680-iaq] (MIT, thstielow); see `NOTICE.md`.
//!
//! **The equivalents are estimates from VOCs.** The sensor sees reducing
//! gases, not CO2; the CO2 equivalent only follows CO2 where people are the
//! source of both.
//!
//! The crate is `no_std`, allocates nothing and doesn't talk to the sensor:
//! measure with any driver (forced mode, heater [`HEATER_TEMPERATURE_C`] for
//! [`HEATER_DURATION_MS`], every [`CYCLE_MS`]) and pass each measurement to
//! [`AirQuality::update`]. With the `bme68x` feature,
//! [`Sample::from_bme68x`] converts that crate's measurements.
//!
//! ```
//! use bme68x_iaq_co2::{AirQuality, Config, Sample};
//!
//! let mut iaq = AirQuality::<100>::new(Config::default());
//! let out = iaq.update(&Sample {
//!     time_ms: 0,
//!     temperature: 24.3,
//!     humidity: 41.0,
//!     gas_ohm: 112_000.0,
//!     gas_valid: true,
//! });
//! // Not calibrated yet: the first minutes are unreliable.
//! assert_eq!(out.air.accuracy, bme68x_iaq_co2::Accuracy::Unreliable);
//!
//! // Keep the calibration across restarts.
//! let mut state = [0u8; AirQuality::<100>::STATE_LEN];
//! iaq.save(&mut state).unwrap();
//! iaq.restore(&state).unwrap();
//! ```
//!
//! [SE_BME680]: https://github.com/steveeidemiller/SE_BME680
//! [raspi-bme680-iaq]: https://github.com/thstielow/raspi-bme680-iaq

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod baseline;
pub mod compensation;
mod donchian;
mod iaq;
mod model;
mod state;

pub use compensation::Climate;
pub use donchian::Donchian;
pub use iaq::{
    Accuracy, Air, AirQuality, Config, Output, Sample, Stage, CYCLE_MS, HEATER_DURATION_MS,
    HEATER_TEMPERATURE_C,
};
pub use model::{Equivalents, Model, MAX_INDEX};
pub use state::StateError;

#[cfg(feature = "bme68x")]
impl Sample {
    /// A sample from a measurement of the `bme68x` driver, taken at
    /// `time_ms`.
    #[must_use]
    pub fn from_bme68x(measurement: &bme68x::Measurement, time_ms: u64) -> Self {
        let values = measurement.values;
        Self {
            time_ms,
            temperature: f32::from(values.temperature) / 100.0,
            humidity: values.humidity as f32 / 1000.0,
            gas_ohm: values.gas_resistance as f32,
            gas_valid: measurement.status.gas_valid() && measurement.status.heater_stable(),
        }
    }
}
