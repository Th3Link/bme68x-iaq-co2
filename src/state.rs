//! The calibration as bytes, to keep it across restarts.
//!
//! Layout (little endian), [`AirQuality::STATE_LEN`] bytes:
//!
//! | Offset | Size | |
//! |---|---|---|
//! | 0 | 4 | magic `IAQS` |
//! | 4 | 1 | version (1) |
//! | 5 | 1 | stage (0 init, 1 burn-in, 2 operation) |
//! | 6 | 2 | `N` |
//! | 8 | 2 | next place in the ceiling |
//! | 10 | 4 | decay periods in operation |
//! | 14 | 4 | ms into the current period |
//! | 18 | 4·`N` | the ceiling's values, f32 |
//! | 18 + 4·`N` | 4 | CRC-32 (IEEE) of everything before |

use crate::baseline::Baseline;
use crate::iaq::{AirQuality, Stage};

const MAGIC: [u8; 4] = *b"IAQS";
const VERSION: u8 = 1;
const HEADER: usize = 18;

/// Why stored bytes were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    /// The buffer is shorter than [`AirQuality::STATE_LEN`].
    Length,
    /// Not a stored state, or of another version.
    Format,
    /// Stored with another `N`.
    Size,
    /// The checksum doesn't match.
    Checksum,
    /// A value is out of range.
    Value,
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Length => "too short for a state",
            Self::Format => "not an air quality state of this version",
            Self::Size => "stored with another number of ceiling values",
            Self::Checksum => "checksum mismatch",
            Self::Value => "value out of range",
        })
    }
}

impl core::error::Error for StateError {}

impl<const N: usize, const W: usize> AirQuality<N, W> {
    /// Bytes of a stored state.
    pub const STATE_LEN: usize = HEADER + 4 * N + 4;

    /// Writes the calibration into `out` and returns its length.
    ///
    /// # Errors
    /// [`StateError::Length`] if `out` is shorter than
    /// [`Self::STATE_LEN`].
    pub fn save(&self, out: &mut [u8]) -> Result<usize, StateError> {
        let out = out.get_mut(..Self::STATE_LEN).ok_or(StateError::Length)?;
        // While warming up after a restore, keep what was restored.
        let (stage, into_period) = match self.resume {
            Some(resume) => resume,
            None => (
                self.stage,
                self.last_ms
                    .map_or(0, |last| last.saturating_sub(self.timer_ms)),
            ),
        };
        out[..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        out[5] = stage as u8;
        out[6..8].copy_from_slice(&(N as u16).to_le_bytes());
        out[8..10].copy_from_slice(&(self.baseline.next as u16).to_le_bytes());
        out[10..14].copy_from_slice(&self.uptime.to_le_bytes());
        out[14..18].copy_from_slice(&(into_period.min(u64::from(u32::MAX)) as u32).to_le_bytes());
        for (chunk, value) in out[HEADER..HEADER + 4 * N]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.baseline.data)
        {
            *chunk = value.to_le_bytes();
        }
        let crc = crc32(&out[..HEADER + 4 * N]);
        out[HEADER + 4 * N..].copy_from_slice(&crc.to_le_bytes());
        Ok(Self::STATE_LEN)
    }

    /// Takes a stored calibration back. The sensor warms up first as after
    /// [`Self::new`] (the air quality is [`crate::Accuracy::Unreliable`]
    /// meanwhile), then continues where it was saved instead of a new
    /// burn-in.
    ///
    /// # Errors
    /// If the bytes aren't a state saved with this `N`; nothing changes then.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let bytes = bytes.get(..Self::STATE_LEN).ok_or(StateError::Length)?;
        if bytes[..4] != MAGIC || bytes[4] != VERSION {
            return Err(StateError::Format);
        }
        if usize::from(u16::from_le_bytes([bytes[6], bytes[7]])) != N {
            return Err(StateError::Size);
        }
        let crc = u32::from_le_bytes(bytes[HEADER + 4 * N..].try_into().unwrap_or_default());
        if crc != crc32(&bytes[..HEADER + 4 * N]) {
            return Err(StateError::Checksum);
        }
        let stage = match bytes[5] {
            0 => Stage::Init,
            1 => Stage::BurnIn,
            2 => Stage::Operation,
            _ => return Err(StateError::Value),
        };
        let mut data = [0.0f32; N];
        for (value, chunk) in data.iter_mut().zip(bytes[HEADER..].as_chunks::<4>().0) {
            *value = f32::from_le_bytes(*chunk);
            if !value.is_finite() || *value < 0.0 {
                return Err(StateError::Value);
            }
        }
        let next = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        let into_period = u64::from(u32::from_le_bytes([
            bytes[14], bytes[15], bytes[16], bytes[17],
        ]));
        let mut restored = Self::new(self.config);
        restored.baseline = Baseline::from_parts(data, next);
        restored.uptime = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        if stage != Stage::Init {
            restored.resume = Some((stage, into_period));
        }
        // Keep the clock, so the warm-up starts now.
        restored.last_ms = self.last_ms;
        restored.timer_ms = self.last_ms.unwrap_or(0);
        *self = restored;
        Ok(())
    }
}

/// CRC-32 (IEEE 802.3, reflected), bitwise: the state is small.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Accuracy, Config, Sample};

    fn sample(time_s: u64, gas: f32) -> Sample {
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

    /// Warm-up, burn-in, and 100 s into operation.
    fn calibrated() -> (AirQuality<4>, u64) {
        let mut iaq = AirQuality::<4>::new(config());
        let mut t = 0;
        for gas in [
            90_000.0, 80_000.0, 70_000.0, 60_000.0, 50_000.0, 51_000.0, 52_000.0, 53_000.0,
        ] {
            iaq.update(&sample(t, gas));
            t += 10;
        }
        let end = t + 400;
        while t < end {
            iaq.update(&sample(t, 100_000.0 + t as f32));
            t += 3;
        }
        assert_eq!(iaq.air().stage, Stage::Operation);
        (iaq, t)
    }

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn round_trip_resumes_after_the_warm_up() {
        let (iaq, _) = calibrated();
        let mut bytes = [0u8; AirQuality::<4>::STATE_LEN];
        assert_eq!(iaq.save(&mut bytes), Ok(AirQuality::<4>::STATE_LEN));

        // A reboot: a new clock, a fresh calculation.
        let mut restored = AirQuality::<4>::new(config());
        restored.restore(&bytes).unwrap();
        let ceiling = iaq.air().ceiling_ohm;
        assert_eq!(restored.air().ceiling_ohm, ceiling);
        assert_eq!(restored.accuracy(), Accuracy::Unreliable);

        let mut t = 0;
        for gas in [60_000.0, 50_000.0, 50_500.0, 51_000.0, 51_500.0] {
            restored.update(&sample(t, gas));
            t += 15;
        }
        assert_eq!(restored.air().stage, Stage::Init);
        restored.update(&sample(t, 52_000.0));
        // Straight back to operation, the ceiling kept.
        assert_eq!(restored.air().stage, Stage::Operation);
        assert_eq!(restored.air().ceiling_ohm, ceiling);
        assert!(restored.accuracy() >= Accuracy::Low);

        // Saving while warming up keeps the restored stage.
        let mut again = AirQuality::<4>::new(config());
        again.restore(&bytes).unwrap();
        again.update(&sample(0, 60_000.0));
        let mut second = [0u8; AirQuality::<4>::STATE_LEN];
        again.save(&mut second).unwrap();
        assert_eq!(second, bytes);
    }

    #[test]
    fn refuses_what_isnt_its_state() {
        let (iaq, _) = calibrated();
        let mut bytes = [0u8; AirQuality::<4>::STATE_LEN];
        iaq.save(&mut bytes).unwrap();
        let mut target = AirQuality::<4>::new(config());

        assert_eq!(target.restore(&bytes[..10]), Err(StateError::Length));
        let mut wrong = bytes;
        wrong[0] = b'X';
        assert_eq!(target.restore(&wrong), Err(StateError::Format));
        let mut wrong = bytes;
        wrong[4] = 2;
        assert_eq!(target.restore(&wrong), Err(StateError::Format));
        let mut wrong = bytes;
        wrong[HEADER] ^= 1;
        assert_eq!(target.restore(&wrong), Err(StateError::Checksum));
        let mut other = AirQuality::<5>::new(config());
        assert_eq!(other.restore(&bytes), Err(StateError::Length));
        let mut bigger = [0u8; AirQuality::<5>::STATE_LEN];
        bigger[..bytes.len()].copy_from_slice(&bytes);
        assert_eq!(other.restore(&bigger), Err(StateError::Size));
        // Nothing changed.
        assert_eq!(target.air().ceiling_ohm, 0.0);
        let mut short = [0u8; 8];
        assert_eq!(iaq.save(&mut short), Err(StateError::Length));
    }

    #[test]
    fn refuses_bad_values_with_a_good_checksum() {
        let (iaq, _) = calibrated();
        let mut bytes = [0u8; AirQuality::<4>::STATE_LEN];
        iaq.save(&mut bytes).unwrap();
        for (offset, value) in [(5, [7u8, 0, 0, 0]), (HEADER, f32::NAN.to_le_bytes())] {
            let mut wrong = bytes;
            let len = if offset == 5 { 1 } else { 4 };
            wrong[offset..offset + len].copy_from_slice(&value[..len]);
            let crc = crc32(&wrong[..HEADER + 16]);
            wrong[HEADER + 16..].copy_from_slice(&crc.to_le_bytes());
            let mut target = AirQuality::<4>::new(config());
            assert_eq!(target.restore(&wrong), Err(StateError::Value));
        }
    }
}
