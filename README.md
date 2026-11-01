# bme68x-iaq-co2

An open indoor air quality calculation for the Bosch **BME680** and **BME688**
gas sensors, in Rust, without Bosch's closed BSEC library. `no_std`, no
allocation, no `unsafe`, independent of the sensor driver.

From temperature, humidity and the gas resistance of each measurement it gives:

| Output | Range | |
|---|---|---|
| Air quality | 0–100 % | 100 % = as clean as the cleanest air the sensor saw |
| Index | 0–500 | 0 = clean, like Bosch's IAQ scale (25 typical good, 250 typical polluted) |
| Breath-VOC equivalent | ppm | estimate |
| CO2 equivalent | ppm | estimate, **not a CO2 measurement** |
| Accuracy | 0–4 | unreliable, low, medium, high, very high |
| Climate | °C, % RH, dew point | with the sensor's self-heating taken out |

## How it works

1. The gas resistance falls exponentially with the absolute humidity. The
   resistance is multiplied by `exp(0.03 × absolute humidity)` to take that
   out.
2. **Warm-up.** The heated sensor's resistance falls for a while. After at
   least 30 s, once it reads above its lowest value three times, it counts as
   settled.
3. **Burn-in.** For at least 5 minutes, and until 100 values are collected,
   the highest compensated resistances form the **gas ceiling** (their mean),
   which stands for clean air. A lower limit keeps a start in bad air from
   being taken as clean.
4. **Operation.**
   - A new high goes into the ceiling right away.
   - After 30 minutes without one, a value is rotated out, so the ceiling
     follows the sensor's drift.
   - The accuracy rises as the ceiling's values come closer together and more
     of these periods pass.
5. The air quality is `(resistance / ceiling)² × 100 %`.
6. The index and the equivalents come from the air quality through a
   documented model (`Model`).
   - A metal-oxide sensor follows `R/R0 = (C/C0)^−β`, so the air quality gives
     the VOC concentration relative to clean air.
   - The index is linear in its logarithm.
   - The breath-VOC equivalent is proportional to the concentration.
   - The CO2 equivalent is proportional to the index, as Bosch describes for
     BSEC.

### Where the numbers come from

Version 0.1 uses published anchor points only:
- Bosch's IAQ scale: 25 is typical good air, 250 typical polluted air.
- BSEC's clean-air output: IAQ 50 ↔ 500 ppm CO2 equivalent ↔ 0.5 ppm breath
  VOC.
- Bosch's statement that the CO2 equivalent is about ten times the static IAQ.
- β = 0.5.

A later version will fit the constants against a real CO2 sensor (Sensirion
SCD30) measuring next to a BME680. All constants are in `Model`, so you can
use your own.

## Usage

Measure in forced mode with any driver. Use the heater at
`HEATER_TEMPERATURE_C` (320 °C) for `HEATER_DURATION_MS` (150 ms), every
`CYCLE_MS` (3 s). **Keep the cycle constant**: the gas resistance depends on
how long the heater rested.

```rust
use bme68x_iaq_co2::{Accuracy, AirQuality, Config, Sample};

let mut iaq = AirQuality::<100>::new(Config {
    // The sensor reads 2.5 K above the room on this board.
    temperature_offset: -2.5,
    ..Config::default()
});

let out = iaq.update(&Sample {
    time_ms: 0,
    temperature: 24.3,
    humidity: 41.0,
    gas_ohm: 112_000.0,
    gas_valid: true,
});
if out.air.accuracy >= Accuracy::Low {
    let e = out.air.equivalents;
    // e.index, e.eco2_ppm, e.bvoc_ppm
}

// Keep the calibration across restarts (422 bytes for 100 values).
let mut state = [0u8; AirQuality::<100>::STATE_LEN];
iaq.save(&mut state).unwrap();
// ... after a restart:
iaq.restore(&state).unwrap();
```

After a restore the sensor warms up again and then carries on with the
restored ceiling, with no new burn-in.

With the `bme68x` feature, `Sample::from_bme68x(&measurement, time_ms)`
converts measurements of the [`bme68x`](https://crates.io/crates/bme68x)
driver.

`AirQuality<N, W>`:
- `N` (default 100) is the number of values in the ceiling.
- `W` (default 0, off) smooths temperature, humidity and gas over the last `W`
  samples (Donchian channel, the middle between minimum and maximum) after the
  warm-up. This helps against heaters or air conditioners cycling.
  `Config::smoothing_limits` caps the channels' widths.

## Limits

- The sensor sees reducing gases (VOCs, CO, H2…). The equivalents assume that
  people are their source. Cleaning agents, cooking or paint raise the "CO2
  equivalent" too, and CO2 from other sources doesn't show.
- The air quality is relative to the cleanest air this sensor saw. It is not
  an absolute concentration.
- The calibration needs hours to days of continuous operation in the same
  place.

## Licence

MIT OR Apache-2.0. The algorithm is a rewrite of
[SE_BME680](https://github.com/steveeidemiller/SE_BME680) (MIT, Steve
Eidemiller), based on
[raspi-bme680-iaq](https://github.com/thstielow/raspi-bme680-iaq) (MIT,
thstielow). See [NOTICE.md](NOTICE.md).

This crate is not affiliated with Bosch Sensortec, and it contains no Bosch
code.
