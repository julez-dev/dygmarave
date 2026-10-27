//! Choosing and opening an audio source that carries what the speakers play.
//!
//! The failure this module exists to prevent is silent and convincing: opening
//! the *default input* captures the microphone, which produces a perfectly valid
//! stream showing the wrong thing. So a source is classified before it is
//! opened, and a microphone is refused rather than recorded.
//!
//! On PipeWire, cpal exposes the default sink's monitor as a capture device
//! called `default_sink`, and every sink as a capturable node, so the monitor is
//! reached without asking PulseAudio for the sink name.

use std::fmt;
use std::str::FromStr;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceDirection, DeviceId, DeviceType};

/// What a capture device actually carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// The output mix — what the speakers play.
    Monitor,
    /// A hardware input, i.e. a microphone. Never what a visualizer wants.
    Microphone,
    /// Not classifiable from the metadata or the name.
    Unknown,
}

impl SourceKind {
    pub const fn describe(self) -> &'static str {
        match self {
            SourceKind::Monitor => "monitor of the output mix",
            SourceKind::Microphone => "microphone input",
            SourceKind::Unknown => "unclassified capture device",
        }
    }
}

/// A capture device that was selected for use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    id: String,
    display_name: String,
    kind: SourceKind,
}

impl Selection {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    /// Whether this device is the sink's monitor handle, which tracks whichever
    /// sink was default when the handle was opened.
    pub fn follows_default_sink(&self) -> bool {
        is_default_sink(&self.id, &self.display_name)
    }
}

impl fmt::Display for Selection {
    /// Always includes the identifier: a device whose description is missing
    /// would otherwise be named as the empty string in error messages.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = if self.display_name.is_empty() {
            "(unnamed)"
        } else {
            &self.display_name
        };
        write!(f, "{name} [{}] ({})", self.id, self.kind.describe())
    }
}

/// A capture device as the host described it, before selection.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub direction: DeviceDirection,
    pub device_type: DeviceType,
}

impl Candidate {
    pub fn kind(&self) -> SourceKind {
        classify(self.direction, self.device_type, &self.id, &self.name)
    }

    /// A candidate with the metadata a name-only host would report.
    #[cfg(test)]
    pub fn from_name(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            direction: DeviceDirection::Unknown,
            device_type: DeviceType::Unknown,
        }
    }
}

#[derive(Debug)]
pub enum AudioError {
    /// No device carries the output mix.
    NoMonitor { candidates: Vec<String> },
    /// The chosen device is a microphone.
    NotAMonitor { selection: Box<Selection> },
    /// The requested device does not exist.
    NoSuchDevice {
        requested: String,
        candidates: Vec<String>,
    },
    /// The audio server or the device refused the stream.
    Device(String),
    /// The stream ran but never carried any signal.
    NoSignal {
        selection: Box<Selection>,
        blocks: u64,
    },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoMonitor { candidates } => write!(
                f,
                "no monitor source is available to capture; found only: {}",
                candidates.join("; ")
            ),
            AudioError::NotAMonitor { selection } => write!(
                f,
                "refusing to capture {selection} because it is a {}; \
                 that would visualize the room, not the speakers",
                selection.kind.describe()
            ),
            AudioError::NoSuchDevice {
                requested,
                candidates,
            } => write!(
                f,
                "no capture device with id {requested:?}; available: {}",
                candidates.join("; ")
            ),
            AudioError::Device(message) => write!(f, "{message}"),
            AudioError::NoSignal { selection, blocks } => write!(
                f,
                "captured {selection} and received {blocks} buffers, but every sample was zero — \
                 is anything playing, and is that sink the one it is playing to?"
            ),
        }
    }
}

impl std::error::Error for AudioError {}

/// Whether captured audio carried anything at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Something audible arrived, at this peak amplitude.
    Heard(f32),
    /// Only digital silence arrived.
    Silent,
}

/// Decides whether capture heard anything.
///
/// Silence means *exact* zero, so a genuinely quiet passage — which still has
/// non-zero samples — is never mistaken for a dead or paused source.
pub fn verdict(loudest: f32) -> Verdict {
    if loudest > 0.0 {
        Verdict::Heard(loudest)
    } else {
        Verdict::Silent
    }
}

/// One buffer of interleaved samples from the capture stream.
pub struct Block {
    pub samples: Vec<f32>,
}

/// An open capture stream. Dropping it stops capture.
pub struct Capture {
    _stream: cpal::Stream,
    pub channels: u16,
    pub sample_rate: u32,
    /// Always f32: any other format is refused at open time, so this cannot
    /// misreport what is being captured.
    pub sample_format: cpal::SampleFormat,
    /// The last stream error, recorded rather than printed: the callback runs on
    /// the audio thread, and writing to stderr from there corrupts a live meter.
    error: Arc<Mutex<Option<String>>>,
}

impl Capture {
    /// Takes the most recent stream error, if the stream failed.
    ///
    /// Capturing a specific sink node fails when that node suspends, which is
    /// normal for an idle sink; capturing the default sink's monitor does not.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Whether a device is the sink's monitor handle.
fn is_default_sink(id: &str, name: &str) -> bool {
    id.to_ascii_lowercase().contains("sink_default") || name.eq_ignore_ascii_case("default_sink")
}

/// Classifies a capture device.
///
/// The host's own metadata decides when it is available: PipeWire exposes a sink
/// as [`DeviceDirection::Duplex`] (capture it and you hear what it plays) and a
/// microphone as [`DeviceDirection::Input`]. Names are only a fallback, because
/// they vary by vendor — a Bluetooth microphone is `bluez_input.*`, not
/// `alsa_input.*`.
pub fn classify(
    direction: DeviceDirection,
    device_type: DeviceType,
    id: &str,
    name: &str,
) -> SourceKind {
    if matches!(device_type, DeviceType::Microphone) {
        return SourceKind::Microphone;
    }
    match direction {
        DeviceDirection::Input => return SourceKind::Microphone,
        DeviceDirection::Duplex => return SourceKind::Monitor,
        // Playback-only nodes cannot be captured at all.
        DeviceDirection::Output => return SourceKind::Unknown,
        // Anything else (including directions added later) falls back to names.
        _ => {}
    }

    let id = id.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    if id.contains("alsa_input")
        || id.contains("bluez_input")
        || id.contains("bluez_source")
        || id.contains("input_default")
        || name == "default_input"
    {
        return SourceKind::Microphone;
    }
    if is_default_sink(&id, &name)
        || id.contains(".monitor")
        || id.contains("alsa_output")
        || id.contains("bluez_output")
    {
        return SourceKind::Monitor;
    }
    SourceKind::Unknown
}

/// Picks a device to capture from.
///
/// Prefers the default sink's monitor so the visualizer follows the user's
/// output choice; never returns a microphone.
pub fn select(candidates: &[Candidate], requested: Option<&str>) -> Result<Selection, AudioError> {
    let described: Vec<(Selection, SourceKind)> = candidates
        .iter()
        .map(|candidate| {
            let kind = candidate.kind();
            (
                Selection {
                    id: candidate.id.clone(),
                    display_name: candidate.name.clone(),
                    kind,
                },
                kind,
            )
        })
        .collect();

    let listing = || {
        candidates
            .iter()
            .map(|candidate| format!("{} [{}]", candidate.name, candidate.id))
            .collect::<Vec<_>>()
    };

    if let Some(requested) = requested {
        let (selection, kind) = described
            .into_iter()
            .find(|(selection, _)| selection.id == requested || selection.display_name == requested)
            .ok_or_else(|| AudioError::NoSuchDevice {
                requested: requested.to_string(),
                candidates: listing(),
            })?;
        if kind == SourceKind::Microphone {
            return Err(AudioError::NotAMonitor {
                selection: Box::new(selection),
            });
        }
        return Ok(selection);
    }

    let monitors: Vec<&(Selection, SourceKind)> = described
        .iter()
        .filter(|(_, kind)| *kind == SourceKind::Monitor)
        .collect();
    let chosen = monitors
        .iter()
        .find(|(selection, _)| selection.follows_default_sink())
        .or_else(|| monitors.first());

    match chosen {
        Some((selection, _)) => Ok(selection.clone()),
        None => Err(AudioError::NoMonitor {
            candidates: listing(),
        }),
    }
}

/// Enumerates the host's capture devices with their metadata.
fn capture_devices() -> Result<Vec<Candidate>, AudioError> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|error| AudioError::Device(error.to_string()))?;
    Ok(devices
        .filter_map(|device| {
            let id = device.id().ok()?.to_string();
            let description = device.description().ok();
            let name = description
                .as_ref()
                .map(|description| description.name().to_string())
                .unwrap_or_default();
            Some(Candidate {
                id,
                name,
                direction: description
                    .as_ref()
                    .map(|description| description.direction())
                    .unwrap_or(DeviceDirection::Unknown),
                device_type: description
                    .as_ref()
                    .map(|description| description.device_type())
                    .unwrap_or(DeviceType::Unknown),
            })
        })
        .collect())
}

/// Selects a source from the devices this host actually offers.
pub fn choose(requested: Option<&str>) -> Result<Selection, AudioError> {
    select(&capture_devices()?, requested)
}

/// Opens a capture stream, delivering interleaved sample blocks to `sink`.
pub fn start(selection: &Selection, sink: Sender<Block>) -> Result<Capture, AudioError> {
    // Defence in depth: selection already refuses microphones, but nothing stops
    // a caller constructing a selection by hand.
    if selection.kind() == SourceKind::Microphone {
        return Err(AudioError::NotAMonitor {
            selection: Box::new(selection.clone()),
        });
    }

    let host = cpal::default_host();
    let device_id = DeviceId::from_str(selection.id())
        .map_err(|_| AudioError::Device(format!("unreadable device id {:?}", selection.id())))?;
    let device = host
        .device_by_id(&device_id)
        .ok_or_else(|| AudioError::NoSuchDevice {
            requested: selection.id().to_string(),
            candidates: Vec::new(),
        })?;

    let supported = device
        .default_input_config()
        .map_err(|error| AudioError::Device(format!("{selection}: {error}")))?;
    // The stream is built for f32 only, so anything else is refused here rather
    // than measured wrongly or reported as a format we are not capturing.
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(AudioError::Device(format!(
            "{selection}: unsupported sample format {:?}; this build captures f32 only",
            supported.sample_format()
        )));
    }
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels;
    let sample_rate = config.sample_rate;

    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let error_writer = Arc::clone(&error_slot);

    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // The channel is unbounded and the consumer may stall (ticket 05
                // will make it far slower than the audio thread). Blocking here
                // would stall the audio callback, so a send that fails because
                // the consumer is gone is simply dropped.
                let _ = sink.send(Block {
                    samples: data.to_vec(),
                });
            },
            move |error| {
                if let Ok(mut slot) = error_writer.lock() {
                    *slot = Some(error.to_string());
                }
            },
            None,
        )
        .map_err(|error| AudioError::Device(error.to_string()))?;
    stream
        .play()
        .map_err(|error| AudioError::Device(error.to_string()))?;

    Ok(Capture {
        _stream: stream,
        channels,
        sample_rate,
        sample_format: cpal::SampleFormat::F32,
        error: error_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, name: &str, direction: DeviceDirection, kind: DeviceType) -> Candidate {
        Candidate {
            id: id.to_string(),
            name: name.to_string(),
            direction,
            device_type: kind,
        }
    }

    fn devices() -> Vec<Candidate> {
        vec![
            candidate(
                "pipewire:sink_default",
                "default_sink",
                DeviceDirection::Duplex,
                DeviceType::Speaker,
            ),
            candidate(
                "pipewire:input_default",
                "default_input",
                DeviceDirection::Input,
                DeviceType::Microphone,
            ),
            candidate(
                "pipewire:alsa_output.usb-Generic_USB_Audio-00.HiFi__Speaker__sink",
                "USB Audio Speakers",
                DeviceDirection::Duplex,
                DeviceType::Speaker,
            ),
            candidate(
                "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback",
                "Auna Mic CM900 Mono",
                DeviceDirection::Input,
                DeviceType::Microphone,
            ),
        ]
    }

    #[test]
    fn classifies_microphones_and_monitors_from_host_metadata() {
        assert_eq!(
            classify(
                DeviceDirection::Input,
                DeviceType::Microphone,
                "pipewire:whatever",
                "Whatever"
            ),
            SourceKind::Microphone
        );
        assert_eq!(
            classify(
                DeviceDirection::Duplex,
                DeviceType::Speaker,
                "pipewire:whatever",
                "Whatever"
            ),
            SourceKind::Monitor
        );
    }

    #[test]
    fn classifies_vendor_specific_microphones_that_name_matching_misses() {
        // A Bluetooth headset's capture node is neither alsa_input nor a monitor.
        let bluetooth_mic = Candidate::from_name(
            "pipewire:bluez_input.00:11:22:33:44:55",
            "Bluetooth Headset",
        );
        assert_eq!(
            classify(
                DeviceDirection::Input,
                DeviceType::Microphone,
                &bluetooth_mic.id,
                &bluetooth_mic.name
            ),
            SourceKind::Microphone
        );
        // ...and the same name matched only by the fallback path.
        assert_eq!(bluetooth_mic.kind(), SourceKind::Microphone);
    }

    #[test]
    fn classifies_a_bluetooth_sink_as_a_monitor() {
        assert_eq!(
            classify(
                DeviceDirection::Duplex,
                DeviceType::Headphones,
                "pipewire:bluez_output.00:11:22:33:44:55",
                "Bluetooth Headphones"
            ),
            SourceKind::Monitor
        );
    }

    #[test]
    fn falls_back_to_names_when_the_host_reports_no_metadata() {
        assert_eq!(
            Candidate::from_name(
                "pulse:alsa_output.pci-0000_01_00.1.hdmi-stereo.monitor",
                "HDMI monitor"
            )
            .kind(),
            SourceKind::Monitor
        );
        assert_eq!(
            Candidate::from_name(
                "pulse:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback",
                "Auna Mic CM900 Mono"
            )
            .kind(),
            SourceKind::Microphone
        );
        assert_eq!(
            Candidate::from_name("pipewire:Firefox", "Firefox").kind(),
            SourceKind::Unknown
        );
    }

    #[test]
    fn prefers_the_default_sink_monitor_over_a_pinned_device() {
        // The default sink comes second in the list; it must still win, so the
        // visualizer follows the user's output choice.
        let mut candidates = devices();
        candidates.swap(0, 2);
        let selection = select(&candidates, None).unwrap();
        assert_eq!(selection.id(), "pipewire:sink_default");
        assert!(selection.follows_default_sink());
    }

    #[test]
    fn never_selects_the_microphone_when_choosing_automatically() {
        let selection = select(&devices(), None).unwrap();
        assert_eq!(selection.kind(), SourceKind::Monitor);
    }

    #[test]
    fn falls_back_to_another_monitor_when_no_default_sink_device_exists() {
        let candidates = vec![
            candidate(
                "pipewire:alsa_output.usb-Generic_USB_Audio-00.HiFi__Speaker__sink",
                "USB Audio Speakers",
                DeviceDirection::Duplex,
                DeviceType::Speaker,
            ),
            candidate(
                "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback",
                "Auna Mic CM900 Mono",
                DeviceDirection::Input,
                DeviceType::Microphone,
            ),
        ];
        let selection = select(&candidates, None).unwrap();
        assert!(selection.id().contains("alsa_output"));
        assert!(!selection.follows_default_sink());
    }

    #[test]
    fn refuses_to_capture_a_microphone_even_when_asked_by_name() {
        let error = select(&devices(), Some("Auna Mic CM900 Mono")).unwrap_err();
        let AudioError::NotAMonitor { selection } = error else {
            panic!("expected a microphone refusal");
        };
        assert_eq!(selection.kind(), SourceKind::Microphone);
        assert!(selection.to_string().contains("microphone"));
    }

    #[test]
    fn refuses_a_bluetooth_microphone_asked_for_by_id() {
        let candidates = vec![candidate(
            "pipewire:bluez_input.00:11:22:33:44:55",
            "Bluetooth Headset",
            DeviceDirection::Input,
            DeviceType::Microphone,
        )];
        let error =
            select(&candidates, Some("pipewire:bluez_input.00:11:22:33:44:55")).unwrap_err();
        assert!(matches!(error, AudioError::NotAMonitor { .. }));
    }

    #[test]
    fn an_explicit_override_may_pick_an_unclassified_device() {
        let candidates = vec![Candidate::from_name("pipewire:42", "Some App")];
        let selection = select(&candidates, Some("pipewire:42")).unwrap();
        assert_eq!(selection.kind(), SourceKind::Unknown);
    }

    #[test]
    fn reports_the_available_devices_when_the_requested_one_is_missing() {
        let error = select(&devices(), Some("pipewire:nope")).unwrap_err();
        let AudioError::NoSuchDevice { candidates, .. } = error else {
            panic!("expected a missing-device error");
        };
        assert!(candidates.iter().any(|line| line.contains("sink_default")));
    }

    #[test]
    fn explains_itself_when_there_is_no_monitor_at_all() {
        let candidates = vec![candidate(
            "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback",
            "Auna Mic CM900 Mono",
            DeviceDirection::Input,
            DeviceType::Microphone,
        )];
        let error = select(&candidates, None).unwrap_err();
        assert!(error.to_string().contains("no monitor source"));
    }

    #[test]
    fn an_unnamed_device_is_still_named_in_errors() {
        let selection = Selection {
            id: "pipewire:42".to_string(),
            display_name: String::new(),
            kind: SourceKind::Monitor,
        };
        assert!(selection.to_string().contains("(unnamed)"));
        assert!(selection.to_string().contains("pipewire:42"));
    }

    #[test]
    fn silence_means_exact_zero_not_a_quiet_passage() {
        assert_eq!(verdict(0.0), Verdict::Silent);
        // -80 dBFS is inaudible but still a signal; it must not be called silent.
        assert!(matches!(verdict(0.0001), Verdict::Heard(_)));
    }
}
