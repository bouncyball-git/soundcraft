//! Audio device selection: which audio system (cpal host), output and input device, sample rate
//! and buffer size to open. The choice is per machine (the app keeps it with its preferences, not
//! in sessions). Anything that cannot be honoured falls back to the system default: a device that
//! is unplugged, a rate or buffer size the device does not offer.

use cpal::traits::{DeviceTrait, HostTrait};

/// Rates offered when a device supports them (any rate inside a supported range is possible, but
/// these are the ones interfaces actually run at).
pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered when the device reports a range that contains them.
pub const STANDARD_BUFFERS: [u32; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// Sample formats the streams convert to and from, best first (ASIO drivers usually run in
/// 32-bit integers, WASAPI and Core Audio in float).
pub(crate) const FORMATS: [cpal::SampleFormat; 3] = [cpal::SampleFormat::F32, cpal::SampleFormat::I32, cpal::SampleFormat::I16];

/// Preference of a sample format (lower is better); `None` when it is not supported.
pub(crate) fn format_rank(f: cpal::SampleFormat) -> Option<usize> {
    FORMATS.iter().position(|x| *x == f)
}

/// The user's audio device choice. Every field is optional: `None` means "the system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio system (cpal host name: "WASAPI", "ASIO", "CoreAudio", "ALSA", "JACK", ...).
    pub system: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    /// Device sample rate; the mix resamples when it differs from the session's.
    pub sample_rate: Option<u32>,
    /// Device buffer in frames.
    pub buffer_size: Option<u32>,
}

impl AudioConfig {
    /// True when the chosen audio system is ASIO.
    pub fn is_asio(&self) -> bool {
        self.system.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(ASIO))
    }

    /// The input device to open. ASIO lets a process load one driver, for input and output alike,
    /// so under ASIO this is the output device whatever `input` says.
    pub fn input_device(&self) -> Option<&str> {
        if self.is_asio() { self.output.as_deref() } else { self.input.as_deref() }
    }
}

/// One device as the settings window lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Most channels any usable configuration offers.
    pub max_channels: u16,
    /// Standard rates the device accepts (in a format the streams convert).
    pub sample_rates: Vec<u32>,
    /// Buffer range the device reports (`None` when it does not say).
    pub buffer_min: Option<u32>,
    pub buffer_max: Option<u32>,
    /// The sizes the driver itself offers (ASIO), ascending; `None` when it does not say.
    pub driver_buffers: Option<Vec<u32>>,
    /// The driver's preferred size (ASIO), also its default.
    pub preferred_buffer: Option<u32>,
}

impl DeviceInfo {
    /// The driver's own sizes when it lists them; else the standard sizes inside the device's
    /// range (all of them when the range is unknown).
    pub fn buffer_sizes(&self) -> Vec<u32> {
        if let Some(sizes) = self.driver_buffers.as_ref().filter(|s| !s.is_empty()) {
            return sizes.clone();
        }
        STANDARD_BUFFERS.iter().copied().filter(|b| self.buffer_min.is_none_or(|m| *b >= m) && self.buffer_max.is_none_or(|m| *b <= m)).collect()
    }

    /// A device known by name only: an ASIO driver this process has not opened (its channels,
    /// rates and buffer sizes are read when it opens).
    pub fn unprobed(name: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            is_default: false,
            max_channels: 0,
            sample_rates: Vec::new(),
            buffer_min: None,
            buffer_max: None,
            driver_buffers: None,
            preferred_buffer: None,
        }
    }

    /// False for a device known by name only ([`DeviceInfo::unprobed`]).
    pub fn has_details(&self) -> bool {
        self.max_channels > 0
    }

    /// The one buffer size a driver takes, when it lists exactly one: its size is set in its own
    /// settings (an RME ASIO driver reports only its current size), not chosen by the host.
    pub fn fixed_buffer(&self) -> Option<u32> {
        match self.driver_buffers.as_deref() {
            Some([only]) => Some(*only),
            _ => None,
        }
    }
}

/// An audio system and its devices.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AudioSystem {
    pub name: String,
    pub is_default: bool,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

/// The device's user-facing name. cpal 0.18 removed `DeviceTrait::name()` in favour of the
/// structured `description()`; `Display` for a device also formats that name but reports a
/// formatting error (which `to_string` would turn into a panic), so keep the fallible call here.
pub(crate) fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|description| description.name().to_owned())
}

/// Describe one cpal device as [`DeviceInfo`], or return `None` when its name or supported configurations cannot be
/// read. Only formats the playback path can convert are considered.
fn describe(d: &cpal::Device, default_name: Option<&str>, output: bool, asio: bool) -> Option<DeviceInfo> {
    let name = device_name(d)?;
    let ranges: Vec<cpal::SupportedStreamConfigRange> = if output {
        d.supported_output_configs().ok()?.filter(|c| format_rank(c.sample_format()).is_some()).collect()
    } else {
        d.supported_input_configs().ok()?.filter(|c| format_rank(c.sample_format()).is_some()).collect()
    };
    let max_channels = ranges.iter().map(cpal::SupportedStreamConfigRange::channels).max().unwrap_or(0);
    let sample_rates = STANDARD_RATES.iter().copied().filter(|r| ranges.iter().any(|c| c.contains_rate(*r))).collect();
    let (mut buffer_min, mut buffer_max) = (None::<u32>, None::<u32>);
    for c in &ranges {
        if let cpal::SupportedBufferSize::Range { min, max } = c.buffer_size() {
            buffer_min = Some(buffer_min.map_or(*min, |m| m.min(*min)));
            buffer_max = Some(buffer_max.map_or(*max, |m| m.max(*max)));
        }
    }
    let driver = if asio { asio_driver_buffers(buffer_min, buffer_max) } else { None };
    let (driver_buffers, preferred_buffer) = driver.map_or((None, None), |(sizes, preferred)| (Some(sizes), Some(preferred)));
    Some(DeviceInfo {
        is_default: default_name == Some(name.as_str()),
        name,
        max_channels,
        sample_rates,
        buffer_min,
        buffer_max,
        driver_buffers,
        preferred_buffer,
    })
}

/// The sizes the loaded ASIO driver offers and its preferred one. cpal keeps a driver loaded while
/// it lists that device, so this asks the driver being described; its range must match the one
/// cpal read from it (`min`, `max`), else nothing is trusted.
#[cfg(feature = "asio")]
fn asio_driver_buffers(min: Option<u32>, max: Option<u32>) -> Option<(Vec<u32>, u32)> {
    let sizes = soundcraft_asio_host::loaded_buffer_sizes().ok()?;
    if Some(sizes.min) != min || Some(sizes.max) != max {
        log::warn!("ASIO driver buffer range {}..={} does not match the listed device; using the standard sizes", sizes.min, sizes.max);
        return None;
    }
    let choices = sizes.choices();
    (!choices.is_empty()).then_some((choices, sizes.preferred))
}

/// The loaded ASIO driver's input and output latency (an open ASIO stream holds the driver).
#[cfg(feature = "asio")]
pub(crate) fn asio_latency() -> Option<crate::DeviceLatency> {
    match soundcraft_asio_host::loaded_latencies() {
        Ok(l) => Some(crate::DeviceLatency { input: l.input, output: l.output }),
        Err(e) => {
            log::warn!("ASIO latency: {e}");
            None
        }
    }
}

#[cfg(not(feature = "asio"))]
pub(crate) fn asio_latency() -> Option<crate::DeviceLatency> {
    None
}

#[cfg(not(feature = "asio"))]
fn asio_driver_buffers(_min: Option<u32>, _max: Option<u32>) -> Option<(Vec<u32>, u32)> {
    None
}

/// cpal's name for the ASIO host.
pub(crate) const ASIO: &str = "ASIO";
/// The audio system name ASIO is listed under.
pub const ASIO_SYSTEM: &str = ASIO;

/// ASIO lets a process load one driver, and loading one probes its hardware (some drivers object
/// when they are opened on their own). So ASIO drivers are listed by registered name, never by
/// loading them, and each driver's details are kept here once this process has opened it.
struct AsioState {
    /// The device the open ASIO streams use, and how many streams use it.
    held: Option<(cpal::Device, usize)>,
    /// Details of the drivers opened so far, by name: as an output and as an input.
    opened: std::collections::BTreeMap<String, (Option<DeviceInfo>, Option<DeviceInfo>)>,
}

static ASIO_STATE: std::sync::Mutex<AsioState> = std::sync::Mutex::new(AsioState { held: None, opened: std::collections::BTreeMap::new() });

fn asio_state() -> std::sync::MutexGuard<'static, AsioState> {
    ASIO_STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Marks an open ASIO stream on a device; dropping it (after the stream) releases the mark.
pub(crate) struct AsioHold;

impl AsioHold {
    /// A hold on `device` when it is an ASIO device (`asio`).
    pub(crate) fn on(asio: bool, device: &cpal::Device) -> Option<AsioHold> {
        if !asio {
            return None;
        }
        let mut st = asio_state();
        st.held = match st.held.take() {
            Some((d, n)) if device_name(&d) == device_name(device) => Some((d, n.saturating_add(1))),
            _ => Some((device.clone(), 1)),
        };
        Some(AsioHold)
    }
}

impl Drop for AsioHold {
    fn drop(&mut self) {
        let mut st = asio_state();
        st.held = match st.held.take() {
            Some((d, n)) if n > 1 => Some((d, n - 1)),
            _ => None,
        };
    }
}

/// Bumped whenever an opened ASIO driver's details change (see [`remember_open_asio_device`]), so
/// a window holding a copy of the listing knows to read it again.
static LISTING_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The version of the device listing; it changes when an open driver's details change.
pub fn listing_version() -> u64 {
    LISTING_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Keep the details of the ASIO driver a stream has just opened: its channels, rates and buffer
/// sizes, read from the open driver (a driver whose size is set in its own settings, RME, reports
/// a new one after a change there). The listing shows them from then on.
pub(crate) fn remember_open_asio_device(device: &cpal::Device) {
    let Some(name) = device_name(device) else { return };
    let details = (describe(device, None, true, true), describe(device, None, false, true));
    let changed = asio_state().opened.insert(name, details.clone()).is_none_or(|old| old != details);
    if changed {
        LISTING_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The registered ASIO drivers' names (no driver is loaded).
#[cfg(feature = "asio")]
fn asio_driver_names() -> Vec<String> {
    soundcraft_asio_host::driver_names()
}

#[cfg(not(feature = "asio"))]
fn asio_driver_names() -> Vec<String> {
    Vec::new()
}

/// The ASIO device an open stream holds, when `name` is it (or no name is asked for).
fn held_asio_device(name: Option<&str>) -> Option<cpal::Device> {
    let st = asio_state();
    let (d, _) = st.held.as_ref()?;
    (name.is_none() || device_name(d).as_deref() == name).then(|| d.clone())
}

/// Every audio system on this machine with its devices. Enumerating can take a moment (some
/// systems probe each device): call it when the settings window opens, not every frame.
pub fn audio_systems() -> Vec<AudioSystem> {
    let default_id = cpal::default_host().id();
    cpal::available_hosts().into_iter().filter_map(|id| list_system(id, default_id)).collect()
}

/// One audio system and its devices.
fn list_system(id: cpal::HostId, default_id: cpal::HostId) -> Option<AudioSystem> {
    if id.name() == ASIO {
        return Some(list_asio(id == default_id));
    }
    let host = cpal::host_from_id(id).ok()?;
    let def_out = host.default_output_device().as_ref().and_then(device_name);
    let def_in = host.default_input_device().as_ref().and_then(device_name);
    let outputs = host.output_devices().map(|ds| ds.filter_map(|d| describe(&d, def_out.as_deref(), true, false)).collect()).unwrap_or_default();
    let inputs = host.input_devices().map(|ds| ds.filter_map(|d| describe(&d, def_in.as_deref(), false, false)).collect()).unwrap_or_default();
    Some(AudioSystem { name: id.name().to_string(), is_default: id == default_id, outputs, inputs })
}

/// The registered ASIO drivers, without loading any: each with the details kept since this process
/// opened it, else by name only. A driver opened without inputs (or outputs) is left out of that
/// side; one never opened is listed on both.
fn list_asio(is_default: bool) -> AudioSystem {
    let names = asio_driver_names();
    let st = asio_state();
    let (mut outputs, mut inputs) = (Vec::new(), Vec::new());
    for name in names {
        match st.opened.get(&name) {
            Some((output, input)) => {
                outputs.extend(output.clone());
                inputs.extend(input.clone());
            }
            None => {
                outputs.push(DeviceInfo::unprobed(&name));
                inputs.push(DeviceInfo::unprobed(&name));
            }
        }
    }
    AudioSystem { name: ASIO.to_string(), is_default, outputs, inputs }
}

/// True when a cpal error means the stream is dead and only a rebuild can restore audio. cpal 0.18
/// reports every operation through one [`cpal::Error`]: `DeviceNotAvailable` means the device went
/// away, and `StreamInvalidated` explicitly requires the stream to be rebuilt. SoundCraft has no
/// in-place rebuild, so both map to the existing actionable "reconnect audio device" trouble
/// instead of leaving a dead stream silently logging. A rerouted default device
/// (`DeviceChanged`) keeps playing, and an xrun is transient, so neither is a lost stream.
/// `HostUnavailable`, `DeviceBusy` and `BackendError` stay log-only until their retry/rebuild
/// policies exist.
pub(crate) fn stream_lost(e: &cpal::Error) -> bool {
    // ErrorKind is non-exhaustive; an equality test avoids a wildcard match arm.
    let kind = e.kind();
    kind == cpal::ErrorKind::DeviceNotAvailable || kind == cpal::ErrorKind::StreamInvalidated
}

/// The device's configuration at `sample_rate` in the best sample format these streams convert.
/// cpal 0.18 ranks `I24` (and `F64`) above `I16` when it picks a default configuration, so a
/// default alone can name a format the callbacks do not decode; this looks for the same rate in a
/// format [`FORMATS`] holds. The exact channel count is preferred; when the device offers none in
/// a usable format at that rate (for example only an 8-channel F32 stream), the closest available
/// channel count is used instead. Both callbacks fold or copy channels, so a wider or narrower
/// device stream is better than falling back to the silent clock. Callers pass a requested count
/// of at least 2 for output; input may ask for mono.
pub(crate) fn supported_format_config(
    configs: &[cpal::SupportedStreamConfigRange],
    sample_rate: u32,
    channels: u16,
) -> Option<cpal::SupportedStreamConfig> {
    let usable = || configs.iter().filter(|c| c.contains_rate(sample_rate)).filter_map(|c| format_rank(c.sample_format()).map(|rank| (rank, c)));
    // Exact count first, then the nearest count (more channels breaks the tie).
    let exact = usable().filter(|(_, c)| c.channels() == channels).min_by_key(|(rank, _)| *rank);
    let best = exact.or_else(|| {
        usable().min_by_key(|(rank, c)| {
            let distance = c.channels().abs_diff(channels);
            (*rank, distance, std::cmp::Reverse(c.channels()))
        })
    });
    best.and_then(|(_, c)| c.try_with_sample_rate(sample_rate))
}

/// The chosen audio system, else the default one.
pub(crate) fn host(cfg: &AudioConfig) -> cpal::Host {
    cfg.system
        .as_deref()
        .and_then(|want| cpal::available_hosts().into_iter().find(|id| id.name().eq_ignore_ascii_case(want)))
        .and_then(|id| cpal::host_from_id(id).ok())
        .unwrap_or_else(|| {
            if let Some(s) = &cfg.system {
                log::warn!("audio system `{s}` is not available; using the default");
            }
            cpal::default_host()
        })
}

/// The named output (or input) device of `host`, else its default device.
pub(crate) fn device(host: &cpal::Host, name: Option<&str>, output: bool) -> Option<cpal::Device> {
    if host.id().name() == ASIO {
        return asio_device(host, name, output);
    }
    if let Some(want) = name {
        let found = if output { host.output_devices().ok() } else { host.input_devices().ok() }
            .and_then(|mut ds| ds.find(|d| device_name(d).as_deref() == Some(want)));
        if found.is_some() {
            return found;
        }
        log::warn!("audio device `{want}` is not available; using the default");
    }
    if output { host.default_output_device() } else { host.default_input_device() }
}

/// The ASIO driver `name` (else the first registered one), loading only that driver: never
/// another as a fallback. Trying drivers in turn loads each, and a driver that cannot open on its
/// own (a member of an aggregate device, hardware that is unplugged) may tell the user so; a
/// driver that cannot open leaves no output (the player runs a silent clock and says so).
fn asio_device(host: &cpal::Host, name: Option<&str>, output: bool) -> Option<cpal::Device> {
    let want = match name {
        Some(n) => n.to_string(),
        None => asio_driver_names().into_iter().next()?,
    };
    let found = host
        .device_by_id(&cpal::DeviceId::new(host.id(), &want))
        .filter(|d| if output { d.supports_output() } else { d.supports_input() })
        // The driver an open stream holds is the only one cpal can open now.
        .or_else(|| held_asio_device(Some(&want)));
    if found.is_none() {
        log::warn!("ASIO driver `{want}` cannot be opened (no other is tried)");
    }
    found
}

/// The buffer to request: the chosen size when the device's range allows it, else the default.
pub(crate) fn buffer_size(want: Option<u32>, supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match (want, supported) {
        (Some(n), cpal::SupportedBufferSize::Range { min, max }) if (*min..=*max).contains(&n) => cpal::BufferSize::Fixed(n),
        // The device does not say: ask anyway (cpal reports an error if it cannot, and the caller
        // falls back to the default).
        (Some(n), cpal::SupportedBufferSize::Unknown) if (16..=8192).contains(&n) => cpal::BufferSize::Fixed(n),
        _ => cpal::BufferSize::Default,
    }
}

/// ASIO drivers take buffer sizes only in steps (`min`, `min + step`, …), and cpal reports the
/// step only in the message of the `UnsupportedConfig` error that refuses another size. The valid
/// size nearest `want` (ties go up: fewer dropouts), or `None` when `e` is not that refusal.
pub(crate) fn stepped_buffer_size(want: cpal::BufferSize, e: &cpal::Error) -> Option<cpal::BufferSize> {
    let cpal::BufferSize::Fixed(want) = want else { return None };
    if e.kind() != cpal::ErrorKind::UnsupportedConfig {
        return None;
    }
    let (_, rest) = e.message()?.split_once("sizes must start at ")?;
    let (min, step) = rest.split_once(" and increment by ")?;
    let min = min.trim().parse::<u32>().ok()?;
    let step = step.trim().parse::<u32>().ok().filter(|s| *s > 0)?;
    let steps = want.saturating_sub(min).saturating_add(step / 2) / step;
    let size = min.checked_add(steps.checked_mul(step)?)?;
    (size != want).then_some(cpal::BufferSize::Fixed(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_tolerates_missing_fields() {
        let c = AudioConfig {
            system: Some("WASAPI".into()),
            output: Some("Speakers".into()),
            input: None,
            sample_rate: Some(48_000),
            buffer_size: Some(256),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<AudioConfig>(&s).unwrap(), c);
        assert_eq!(serde_json::from_str::<AudioConfig>("{}").unwrap(), AudioConfig::default());
        assert_eq!(serde_json::from_str::<AudioConfig>(r#"{"output":"X","unknown":1}"#).unwrap().output.as_deref(), Some("X"));
    }

    /// Build a cpal configuration range for tests with a 64..1024-frame buffer.
    fn range(channels: u16, min: u32, max: u32, format: cpal::SampleFormat) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(channels, min, max, cpal::SupportedBufferSize::Range { min: 64, max: 1024 }, format)
    }

    /// When the default sample format has no usable conversion, the same rate opens with a supported format; the
    /// nearest channel count keeps the device open, and unsupported rates fail.
    #[test]
    fn a_default_format_the_streams_cannot_convert_uses_a_supported_one_at_the_same_rate() {
        let configs = [
            range(2, 44_100, 48_000, cpal::SampleFormat::I24),
            range(2, 44_100, 48_000, cpal::SampleFormat::I16),
            range(2, 44_100, 48_000, cpal::SampleFormat::F32),
            range(2, 96_000, 96_000, cpal::SampleFormat::F32),
        ];
        let c = supported_format_config(&configs, 48_000, 2).unwrap();
        assert_eq!((c.sample_rate(), c.channels(), c.sample_format()), (48_000, 2, cpal::SampleFormat::F32));
        assert_eq!(supported_format_config(&configs, 96_000, 2).map(|c| c.sample_format()), Some(cpal::SampleFormat::F32));
        assert_eq!(
            supported_format_config(&configs, 48_000, 1).map(|c| (c.channels(), c.sample_format())),
            Some((2, cpal::SampleFormat::F32)),
            "with no mono stream, the nearest usable channel count keeps the device open"
        );
        let i24_only = [range(2, 44_100, 48_000, cpal::SampleFormat::I24)];
        assert!(supported_format_config(&i24_only, 48_000, 2).is_none(), "no supported format at this rate");
    }

    /// cpal 0.18 may name an I24 stereo default while the only format the callbacks decode is a
    /// wider F32 stream at another supported rate. The default-rate search finds nothing usable,
    /// so the output fallback retries at the session rate over any usable channel count; the mix
    /// already folds or copies channels and resamples.
    #[test]
    fn a_default_format_with_no_usable_same_rate_stream_falls_back_to_the_session_rate() {
        let configs = [range(2, 44_100, 44_100, cpal::SampleFormat::I24), range(8, 48_000, 48_000, cpal::SampleFormat::F32)];
        assert!(supported_format_config(&configs, 44_100, 2).is_none(), "the default rate has no usable format");
        let c = supported_format_config(&configs, 48_000, 2).expect("the wider F32 stream at the session rate");
        assert_eq!((c.sample_rate(), c.channels(), c.sample_format()), (48_000, 8, cpal::SampleFormat::F32));
    }

    /// Only device-not-available and invalidated-stream errors stop playback; other cpal errors are treated as
    /// recoverable.
    #[test]
    fn only_a_dead_stream_is_reported() {
        let error = |kind| cpal::Error::with_message(kind, "test");
        assert!(stream_lost(&error(cpal::ErrorKind::DeviceNotAvailable)));
        assert!(stream_lost(&error(cpal::ErrorKind::StreamInvalidated)), "the stream must be rebuilt");
        for kind in [
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::DeviceBusy,
            cpal::ErrorKind::HostUnavailable,
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::BackendError,
        ] {
            assert!(!stream_lost(&error(kind)), "{kind:?} must not stop the stream");
        }
    }

    #[test]
    fn buffer_sizes_follow_the_device_range() {
        let r = cpal::SupportedBufferSize::Range { min: 64, max: 1024 };
        assert_eq!(buffer_size(Some(256), &r), cpal::BufferSize::Fixed(256));
        assert_eq!(buffer_size(Some(32), &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(Some(4096), &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(None, &r), cpal::BufferSize::Default);
    }

    #[test]
    fn a_stepped_asio_buffer_refusal_picks_the_nearest_valid_size() {
        use cpal::BufferSize::Fixed;
        // cpal 0.18's wording for a driver whose sizes go 48, 96, 144, …
        let refused =
            cpal::Error::with_message(cpal::ErrorKind::UnsupportedConfig, "Buffer size 128 is not valid; sizes must start at 48 and increment by 48");
        assert_eq!(stepped_buffer_size(Fixed(128), &refused), Some(Fixed(144)));
        assert_eq!(stepped_buffer_size(Fixed(110), &refused), Some(Fixed(96)));
        assert_eq!(stepped_buffer_size(Fixed(120), &refused), Some(Fixed(144)), "a tie goes up");
        assert_eq!(stepped_buffer_size(Fixed(16), &refused), Some(Fixed(48)), "below the minimum");
        assert_eq!(stepped_buffer_size(Fixed(96), &refused), None, "already valid: nothing new to try");
        assert_eq!(stepped_buffer_size(cpal::BufferSize::Default, &refused), None);
        // Other refusals and other kinds are not a step.
        let range = cpal::Error::with_message(cpal::ErrorKind::UnsupportedConfig, "Buffer size 9000 is not in the supported range 48..=4096");
        assert_eq!(stepped_buffer_size(Fixed(9000), &range), None);
        let busy = cpal::Error::with_message(cpal::ErrorKind::DeviceBusy, "sizes must start at 48 and increment by 48");
        assert_eq!(stepped_buffer_size(Fixed(128), &busy), None);
        let zero = cpal::Error::with_message(cpal::ErrorKind::UnsupportedConfig, "sizes must start at 48 and increment by 0");
        assert_eq!(stepped_buffer_size(Fixed(128), &zero), None);
        let huge = cpal::Error::with_message(cpal::ErrorKind::UnsupportedConfig, "sizes must start at 4294967295 and increment by 7");
        assert_eq!(stepped_buffer_size(Fixed(128), &huge), Some(Fixed(u32::MAX)));
        assert_eq!(buffer_size(Some(128), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Fixed(128));
        assert_eq!(buffer_size(Some(1 << 20), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Default);
        let d = DeviceInfo {
            name: "x".into(),
            is_default: true,
            max_channels: 2,
            sample_rates: vec![48_000],
            buffer_min: Some(64),
            buffer_max: Some(1024),
            driver_buffers: None,
            preferred_buffer: None,
        };
        assert_eq!(d.buffer_sizes(), [64, 128, 256, 512, 1024]);
        let open = DeviceInfo { buffer_min: None, buffer_max: None, ..d.clone() };
        assert_eq!(open.buffer_sizes().len(), STANDARD_BUFFERS.len());
        // A driver's own list wins; an empty one falls back to the standard sizes.
        let asio = DeviceInfo { driver_buffers: Some(vec![48, 96, 144]), preferred_buffer: Some(96), ..d.clone() };
        assert_eq!(asio.buffer_sizes(), [48, 96, 144]);
        let empty = DeviceInfo { driver_buffers: Some(Vec::new()), ..d.clone() };
        assert_eq!(empty.buffer_sizes(), [64, 128, 256, 512, 1024]);
        // A driver that lists one size (set in its own settings) has a fixed buffer; a range does not.
        assert_eq!(DeviceInfo { driver_buffers: Some(vec![64]), ..d.clone() }.fixed_buffer(), Some(64));
        assert_eq!(asio.fixed_buffer(), None);
        assert_eq!(d.fixed_buffer(), None, "no driver list: not fixed");
        assert_eq!(empty.fixed_buffer(), None);
        // Known by name only: no details, the standard sizes, nothing fixed.
        let named = DeviceInfo::unprobed("Some ASIO driver");
        assert!(!named.has_details() && d.has_details());
        assert_eq!(named.buffer_sizes(), STANDARD_BUFFERS);
        assert_eq!(named.fixed_buffer(), None);
    }

    #[test]
    fn asio_opens_its_input_on_the_output_driver() {
        let wasapi = AudioConfig { system: Some("WASAPI".into()), output: Some("Speakers".into()), input: Some("Mic".into()), ..Default::default() };
        assert!(!wasapi.is_asio());
        assert_eq!(wasapi.input_device(), Some("Mic"));
        let asio = AudioConfig { system: Some("asio".into()), output: Some("Driver A".into()), input: Some("Driver B".into()), ..Default::default() };
        assert!(asio.is_asio());
        assert_eq!(asio.input_device(), Some("Driver A"), "one driver for input and output");
        let default = AudioConfig { system: Some("ASIO".into()), ..Default::default() };
        assert_eq!(default.input_device(), None);
    }

    #[test]
    fn unknown_systems_and_devices_fall_back_to_the_defaults() {
        let cfg = AudioConfig { system: Some("No Such System".into()), ..Default::default() };
        assert_eq!(host(&cfg).id(), cpal::default_host().id());
        // No device with this name exists: the default (or none, on a machine without audio).
        let h = cpal::default_host();
        let fallback = device(&h, Some("No Such Device 1234"), true).as_ref().and_then(device_name);
        assert_eq!(fallback, h.default_output_device().as_ref().and_then(device_name));
    }

    #[test]
    fn enumeration_does_not_fail() {
        let systems = audio_systems();
        // Every listed system has a name; on a machine without audio the lists are just empty.
        assert!(systems.iter().all(|s| !s.name.is_empty()));
        assert!(systems.iter().filter(|s| s.is_default).count() <= 1);
    }
}
