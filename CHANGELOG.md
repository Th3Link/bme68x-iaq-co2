# Changelog

## 0.1.0 (unreleased)

- First version. It provides:
  - The air quality from humidity-compensated gas resistance against a
    self-calibrating gas ceiling (rewrite of `SE_BME680`).
  - An index 0–500, plus breath-VOC and CO2 equivalents from published
    anchor points.
  - Accuracy levels and optional Donchian smoothing.
  - The climate with a temperature offset.
  - A stored state.
  - An optional `bme68x` driver conversion.
