//! Humidity and temperature formulas (Magnus, over water).
//!
//! The gas resistance of a metal-oxide sensor falls exponentially with the
//! absolute humidity; [`gas_factor`] undoes that, as in raspi-bme680-iaq and
//! `SE_BME680`.

use libm::{expf, logf};

/// Magnus coefficients over water (Sonntag 1990, as used by `SE_BME680`).
const MAGNUS_B: f32 = 17.625;
const MAGNUS_C: f32 = 243.04;
/// Saturation vapour pressure at 0 °C, in hPa.
const MAGNUS_E0: f32 = 6.112;
/// Specific gas constant of water vapour, in J/(kg·K).
const R_WATER: f32 = 461.52;
/// 0 °C in kelvin.
const ZERO_C: f32 = 273.15;

/// Humidity below this is treated as this (the logarithms need a value
/// above zero).
const MIN_HUMIDITY: f32 = 0.01;

/// Saturation vapour pressure in hPa at `temperature` (°C).
#[must_use]
pub fn saturation_vapour_pressure(temperature: f32) -> f32 {
    MAGNUS_E0 * expf(MAGNUS_B * temperature / (MAGNUS_C + temperature))
}

/// Saturation water vapour density in kg/m³ at `temperature` (°C): the
/// absolute humidity at 100 % RH.
#[must_use]
pub fn saturation_vapour_density(temperature: f32) -> f32 {
    saturation_vapour_pressure(temperature) * 100.0 / (R_WATER * (temperature + ZERO_C))
}

/// Absolute humidity in g/m³ from `temperature` (°C) and `humidity` (% RH).
#[must_use]
pub fn absolute_humidity(temperature: f32, humidity: f32) -> f32 {
    // % RH × 10 × kg/m³ = RH/100 × 1000 g/kg × kg/m³.
    humidity * 10.0 * saturation_vapour_density(temperature)
}

/// The factor that takes the humidity out of a gas resistance:
/// `exp(slope × absolute humidity)`.
#[must_use]
pub fn gas_factor(temperature: f32, humidity: f32, slope: f32) -> f32 {
    expf(slope * absolute_humidity(temperature, humidity))
}

/// Dew point in °C (Magnus).
#[must_use]
pub fn dew_point(temperature: f32, humidity: f32) -> f32 {
    let gamma = logf(humidity.max(MIN_HUMIDITY) / 100.0)
        + MAGNUS_B * temperature / (MAGNUS_C + temperature);
    MAGNUS_C * gamma / (MAGNUS_B - gamma)
}

/// Temperature, humidity and dew point of the air around the sensor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Climate {
    /// °C, with the offset applied.
    pub temperature: f32,
    /// % RH at that temperature (0–100).
    pub humidity: f32,
    /// °C. The offset doesn't change it: the water in the air is the same.
    pub dew_point: f32,
}

/// The climate with the sensor's self-heating taken out: `offset` (°C,
/// usually negative) is added to the temperature, and the relative humidity
/// is the same amount of water at that temperature.
#[must_use]
pub fn climate(temperature: f32, humidity: f32, offset: f32) -> Climate {
    let corrected = temperature + offset;
    let vapour = humidity / 100.0 * saturation_vapour_pressure(temperature);
    Climate {
        temperature: corrected,
        humidity: (vapour / saturation_vapour_pressure(corrected) * 100.0).clamp(0.0, 100.0),
        dew_point: dew_point(temperature, humidity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn saturation_vapour_pressure_matches_tables() {
        // Reference values (hPa) over water.
        assert!(close(saturation_vapour_pressure(0.0), 6.112, 0.001));
        assert!(close(saturation_vapour_pressure(20.0), 23.37, 0.05));
        assert!(close(saturation_vapour_pressure(30.0), 42.43, 0.1));
    }

    #[test]
    fn absolute_humidity_matches_tables() {
        // 20 °C, 100 % RH: 17.3 g/m³; 25 °C, 50 %: 11.5 g/m³.
        assert!(close(absolute_humidity(20.0, 100.0), 17.3, 0.1));
        assert!(close(absolute_humidity(25.0, 50.0), 11.5, 0.1));
        assert_eq!(absolute_humidity(25.0, 0.0), 0.0);
    }

    #[test]
    fn gas_factor_is_exponential_in_absolute_humidity() {
        let dry = gas_factor(25.0, 0.0, 0.03);
        assert_eq!(dry, 1.0);
        let humid = gas_factor(25.0, 50.0, 0.03);
        assert!(close(
            humid,
            expf(0.03 * absolute_humidity(25.0, 50.0)),
            1e-6
        ));
        assert!(gas_factor(25.0, 80.0, 0.03) > humid);
    }

    #[test]
    fn dew_point_matches_tables() {
        assert!(close(dew_point(20.0, 100.0), 20.0, 0.01));
        assert!(close(dew_point(20.0, 50.0), 9.3, 0.1));
        assert!(close(dew_point(30.0, 70.0), 23.9, 0.1));
        // No NaN at 0 % RH.
        assert!(dew_point(20.0, 0.0).is_finite());
    }

    #[test]
    fn climate_keeps_the_water() {
        // The sensor reads 29 °C / 40 %; the room is 4 K cooler.
        let c = climate(29.0, 40.0, -4.0);
        assert_eq!(c.temperature, 25.0);
        assert!(c.humidity > 40.0);
        // Same vapour pressure, so the same dew point.
        assert!(close(
            dew_point(c.temperature, c.humidity),
            c.dew_point,
            0.01
        ));
        // Without an offset nothing changes.
        let same = climate(21.0, 55.0, 0.0);
        assert!(close(same.humidity, 55.0, 1e-4));
        // Clamped at saturation.
        assert_eq!(climate(25.0, 95.0, -10.0).humidity, 100.0);
    }
}
