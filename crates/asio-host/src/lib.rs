//! SoundCraft's ASIO driver helpers.
//!
//! cpal opens ASIO streams but does not expose two things a DAW needs from the driver: the buffer
//! sizes it accepts (cpal reports only the minimum and maximum, not the driver's preferred size or
//! its step) and the driver's own control panel. Both are ASIO SDK calls on the *loaded* driver.
//! ASIO allows one loaded driver per process; cpal loads it (through asio-sys) while it lists a
//! device and while a stream is open, so these helpers act on that driver and never load one.
//!
//! Like `soundcraft-au-host`, this is an isolated `unsafe` crate (craftrules never-crash):
//! `unsafe` is denied crate-wide and allowed only in the private `ffi` module (two declarations
//! of ASIO SDK functions compiled by asio-sys). The public API is safe, returns `Result` and never
//! panics.
//!
//! Off Windows with the MSVC toolchain the crate compiles to stubs that report "unsupported".

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(windows, target_env = "msvc"))]
#[allow(unsafe_code)]
mod ffi;

/// What the loaded driver reports about its buffer sizes, in frames (`ASIOGetBufferSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSizes {
    pub min: u32,
    pub max: u32,
    pub preferred: u32,
    /// `-1`: powers of two from `min` to `max`; `0`: only `preferred` (`min == max`); `n > 0`:
    /// `min`, `min + n`, … up to `max`.
    pub granularity: i32,
}

/// Most entries [`BufferSizes::choices`] lists before it keeps only the usual sizes.
pub const MAX_CHOICES: usize = 64;

/// Sizes kept when a fine step would make the list too long (each is snapped to the step).
const USUAL: [u32; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

impl BufferSizes {
    /// The sizes to offer, ascending, always including `preferred` (when it is in range). A step
    /// so fine that it would give more than [`MAX_CHOICES`] sizes offers the usual sizes snapped
    /// to the step, plus `min`, `preferred` and the largest size on the step (`max` itself may be
    /// off it).
    pub fn choices(&self) -> Vec<u32> {
        let (min, max) = (self.min, self.max);
        if min == 0 || min > max {
            return Vec::new();
        }
        let mut out: Vec<u32> = match self.granularity {
            -1 => (0..32).filter_map(|p| 1u32.checked_shl(p)).filter(|s| (min..=max).contains(s)).collect(),
            step if step > 0 => {
                let step = step.unsigned_abs();
                let steps = (max - min) / step;
                let top = min + steps * step;
                if (steps as usize) < MAX_CHOICES {
                    (0..=steps).map(|i| min + i * step).collect()
                } else {
                    // Nearest step, but never past the top one.
                    let snap = |s: u32| min.saturating_add(s.saturating_sub(min).saturating_add(step / 2) / step * step).min(top);
                    USUAL.iter().copied().filter(|s| (min..=max).contains(s)).map(snap).chain([min, top]).collect()
                }
            }
            _ => Vec::new(),
        };
        if (min..=max).contains(&self.preferred) {
            out.push(self.preferred);
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Why a driver call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsioError {
    /// No ASIO driver is loaded (nothing is using ASIO right now).
    NotLoaded,
    /// The driver returned this ASIO error code.
    Driver(i32),
    /// Not a Windows MSVC build.
    Unsupported,
}

impl std::fmt::Display for AsioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoaded => f.write_str("no ASIO driver is in use"),
            Self::Driver(code) => write!(f, "the ASIO driver reported error {code}"),
            Self::Unsupported => f.write_str("ASIO is not available in this build"),
        }
    }
}

impl std::error::Error for AsioError {}

/// The loaded driver's buffer sizes.
pub fn loaded_buffer_sizes() -> Result<BufferSizes, AsioError> {
    #[cfg(all(windows, target_env = "msvc"))]
    {
        let (min, max, preferred, granularity) = ffi::get_buffer_size().map_err(code_error)?;
        let frames = |n: i32| u32::try_from(n).map_err(|_| AsioError::Driver(n));
        Ok(BufferSizes { min: frames(min)?, max: frames(max)?, preferred: frames(preferred)?, granularity })
    }
    #[cfg(not(all(windows, target_env = "msvc")))]
    {
        Err(AsioError::Unsupported)
    }
}

/// Open the loaded driver's control panel. Most drivers show it without blocking; a change there
/// usually ends with the driver asking the host to reset its streams.
pub fn open_control_panel() -> Result<(), AsioError> {
    #[cfg(all(windows, target_env = "msvc"))]
    {
        ffi::control_panel().map_err(code_error)
    }
    #[cfg(not(all(windows, target_env = "msvc")))]
    {
        Err(AsioError::Unsupported)
    }
}

/// `ASE_NotPresent`: the SDK's answer when no driver is loaded.
#[cfg(all(windows, target_env = "msvc"))]
fn code_error(code: i32) -> AsioError {
    if code == ffi::ASE_NOT_PRESENT { AsioError::NotLoaded } else { AsioError::Driver(code) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(min: u32, max: u32, preferred: u32, granularity: i32) -> Vec<u32> {
        BufferSizes { min, max, preferred, granularity }.choices()
    }

    #[test]
    fn a_stepped_driver_offers_every_step_with_its_preferred_size() {
        assert_eq!(sizes(48, 480, 240, 48), vec![48, 96, 144, 192, 240, 288, 336, 384, 432, 480]);
    }

    #[test]
    fn a_power_of_two_driver_offers_powers_of_two() {
        assert_eq!(sizes(64, 2048, 256, -1), vec![64, 128, 256, 512, 1024, 2048]);
        assert_eq!(sizes(48, 1000, 96, -1), vec![64, 96, 128, 256, 512], "the preferred size is kept even off the grid");
    }

    #[test]
    fn a_fixed_driver_offers_only_its_preferred_size() {
        assert_eq!(sizes(512, 512, 512, 0), vec![512]);
    }

    #[test]
    fn a_fine_step_keeps_the_usual_sizes_snapped_to_it() {
        // 48..=12288 in steps of 48 would be 256 entries.
        let c = sizes(48, 12_288, 480, 48);
        assert_eq!(c, vec![48, 144, 240, 480, 528, 1008, 2064, 4080, 8208, 12_288]);
        // A maximum off the step: the top entry is the largest size on it.
        let c = sizes(48, 8192, 480, 48);
        assert_eq!(c, vec![48, 144, 240, 480, 528, 1008, 2064, 4080, 8160]);
        assert!(c.iter().all(|s| (s - 48) % 48 == 0), "{c:?}");
        assert_eq!(sizes(48, 1000, 48, 48).last(), Some(&960));
        // A step of one frame: the usual sizes themselves.
        assert_eq!(sizes(32, 4096, 256, 1), vec![32, 64, 128, 256, 512, 1024, 2048, 4096]);
    }

    #[test]
    fn nonsense_reports_offer_nothing() {
        assert!(sizes(0, 512, 256, 32).is_empty());
        assert!(sizes(1024, 512, 512, 32).is_empty());
        assert_eq!(sizes(64, 512, 9000, -7), Vec::<u32>::new());
        // Extreme values neither overflow nor loop forever.
        assert_eq!(sizes(u32::MAX - 1, u32::MAX, u32::MAX, i32::MAX), vec![u32::MAX - 1, u32::MAX]);
    }
}
