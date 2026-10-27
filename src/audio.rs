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
use cpal::DeviceId;

/// What a capture device actually carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// The output mix — what the speakers play.
    Monitor,
    /// A hardware input, i.e. a microphone. Never what a visualizer wants.
    Microphone,
    /// Not classifiable from its name.
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
    /// The cpal device identifier, which is also the PipeWire/PulseAudio node name.
    pub id: String,
    /// Human-readable name, for logs.
    pub display_name: String,
    pub kind: SourceKind,
}

impl Selection {
    /// Whether this device is the default sink's monitor, which follows the
    /// user's output choice instead of pinning one device.
    pub fn follows_default_sink(&self) -> bool {
        self.id.contains("sink_default") || self.display_name == "default_sink"
    }
}

impl fmt::Display for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}, {})",
            self.display_name,
            self.id,
            self.kind.describe()
        )
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
    NoSignal { selection: Box<Selection> },
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
                "refusing to capture {} because it is a {}; \
                 that would visualize the room, not the speakers",
                selection.display_name,
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
            AudioError::NoSignal { selection } => write!(
                f,
                "captured {} but heard nothing — is anything playing, \
                 and is that sink the one it is playing to?",
                selection.display_name
            ),
        }
    }
}

impl std::error::Error for AudioError {}

/// One buffer of interleaved samples from the capture stream.
pub struct Block {
    pub samples: Vec<f32>,
}

/// An open capture stream. Dropping it stops capture.
pub struct Capture {
    _stream: cpal::Stream,
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: cpal::SampleFormat,
    /// The last stream error, recorded rather than printed: the callback runs on
    /// the audio thread, and writing to stderr from there corrupts a live meter.
    error: Arc<Mutex<Option<String>>>,
}

impl Capture {
    /// Takes the most recent stream error, if the stream failed.
    ///
    /// Capturing a specific sink node fails when that node suspends, which is
    /// normal for an idle sink; capturing `default_sink` does not.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Classifies a capture device from its identifier and name.
///
/// PipeWire and PulseAudio name things predictably: hardware inputs are
/// `alsa_input.*`, sinks are `alsa_output.*`, and a sink's monitor either *is*
/// the sink node or ends in `.monitor`.
pub fn classify(id: &str, name: &str) -> SourceKind {
    let id = id.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();

    if id.contains("alsa_input") || id.contains("input_default") || name == "default_input" {
        return SourceKind::Microphone;
    }
    if id.contains("sink_default")
        || name == "default_sink"
        || id.contains(".monitor")
        || id.contains("alsa_output")
    {
        return SourceKind::Monitor;
    }
    SourceKind::Unknown
}

/// Picks a device to capture from, given the devices the host offers.
///
/// Prefers the default sink's monitor so the visualizer follows the user's
/// output choice; never returns a microphone.
pub fn select(
    candidates: &[(String, String)],
    requested: Option<&str>,
) -> Result<Selection, AudioError> {
    let described: Vec<(String, String, SourceKind)> = candidates
        .iter()
        .map(|(id, name)| (id.clone(), name.clone(), classify(id, name)))
        .collect();

    if let Some(requested) = requested {
        return described
            .into_iter()
            .find(|(id, name, _)| id == requested || name == requested)
            .map(|(id, display_name, kind)| Selection {
                id,
                display_name,
                kind,
            })
            .ok_or_else(|| AudioError::NoSuchDevice {
                requested: requested.to_string(),
                candidates: candidates
                    .iter()
                    .map(|(id, name)| format!("{name} [{id}]"))
                    .collect(),
            })
            .and_then(|selection| {
                if selection.kind == SourceKind::Microphone {
                    Err(AudioError::NotAMonitor {
                        selection: Box::new(selection),
                    })
                } else {
                    Ok(selection)
                }
            });
    }

    let monitors: Vec<&(String, String, SourceKind)> = described
        .iter()
        .filter(|(_, _, kind)| *kind == SourceKind::Monitor)
        .collect();

    let chosen = monitors
        .iter()
        .find(|(id, name, _)| id.contains("sink_default") || name == &"default_sink".to_string())
        .or_else(|| monitors.first());

    match chosen {
        Some((id, display_name, kind)) => Ok(Selection {
            id: id.clone(),
            display_name: display_name.clone(),
            kind: *kind,
        }),
        None => Err(AudioError::NoMonitor {
            candidates: candidates
                .iter()
                .map(|(id, name)| format!("{name} [{id}]"))
                .collect(),
        }),
    }
}

/// Enumerates the host's capture devices.
fn capture_devices() -> Result<Vec<(String, String)>, AudioError> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|error| AudioError::Device(error.to_string()))?;
    Ok(devices
        .filter_map(|device| {
            let id = device.id().ok()?.to_string();
            let name = device
                .description()
                .map(|description| description.name().to_string())
                .unwrap_or_default();
            Some((id, name))
        })
        .collect())
}

/// Selects a source from the devices this host actually offers.
pub fn choose(requested: Option<&str>) -> Result<Selection, AudioError> {
    select(&capture_devices()?, requested)
}

/// Opens a capture stream, delivering interleaved sample blocks to `sink`.
pub fn start(selection: &Selection, sink: Sender<Block>) -> Result<Capture, AudioError> {
    let host = cpal::default_host();
    let device_id = DeviceId::from_str(&selection.id)
        .map_err(|_| AudioError::Device(format!("unreadable device id {:?}", selection.id)))?;
    let device = host
        .device_by_id(&device_id)
        .ok_or_else(|| AudioError::NoSuchDevice {
            requested: selection.id.clone(),
            candidates: Vec::new(),
        })?;

    let supported = device
        .default_input_config()
        .map_err(|error| AudioError::Device(format!("{}: {error}", selection.display_name)))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels;
    let sample_rate = config.sample_rate;

    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let error_writer = Arc::clone(&error_slot);

    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // A full channel is gone: the consumer stops reading long before
                // this matters, and blocking the audio thread is worse than a
                // dropped block.
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
        sample_format,
        error: error_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<(String, String)> {
        vec![
            (
                "pipewire:sink_default".to_string(),
                "default_sink".to_string(),
            ),
            (
                "pipewire:input_default".to_string(),
                "default_input".to_string(),
            ),
            (
                "pipewire:alsa_output.usb-Generic_USB_Audio-00.HiFi__Speaker__sink".to_string(),
                "USB Audio Speakers".to_string(),
            ),
            (
                "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback".to_string(),
                "Auna Mic CM900 Mono".to_string(),
            ),
        ]
    }

    #[test]
    fn classifies_microphones_and_monitors() {
        assert_eq!(
            classify(
                "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback",
                "Auna Mic CM900 Mono"
            ),
            SourceKind::Microphone
        );
        assert_eq!(
            classify("pipewire:input_default", "default_input"),
            SourceKind::Microphone
        );
        assert_eq!(
            classify("pipewire:sink_default", "default_sink"),
            SourceKind::Monitor
        );
        assert_eq!(
            classify(
                "pipewire:alsa_output.usb-Generic_USB_Audio-00.HiFi__Speaker__sink",
                "USB Audio Speakers"
            ),
            SourceKind::Monitor
        );
        // PulseAudio names a sink's monitor explicitly.
        assert_eq!(
            classify(
                "pulse:alsa_output.pci-0000_01_00.1.hdmi-stereo.monitor",
                "HDMI monitor"
            ),
            SourceKind::Monitor
        );
        assert_eq!(classify("pipewire:Firefox", "Firefox"), SourceKind::Unknown);
    }

    #[test]
    fn prefers_the_default_sink_monitor_over_a_pinned_device() {
        // The default sink comes second in the list; it must still win, so the
        // visualizer follows the user's output choice.
        let mut candidates = devices();
        candidates.swap(0, 2);
        let selection = select(&candidates, None).unwrap();
        assert_eq!(selection.id, "pipewire:sink_default");
        assert!(selection.follows_default_sink());
    }

    #[test]
    fn never_selects_the_microphone_when_choosing_automatically() {
        let selection = select(&devices(), None).unwrap();
        assert_eq!(selection.kind, SourceKind::Monitor);
    }

    #[test]
    fn falls_back_to_another_monitor_when_no_default_sink_device_exists() {
        let candidates = vec![
            (
                "pipewire:alsa_output.usb-Generic_USB_Audio-00.HiFi__Speaker__sink".to_string(),
                "USB Audio Speakers".to_string(),
            ),
            (
                "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback".to_string(),
                "Auna Mic CM900 Mono".to_string(),
            ),
        ];
        let selection = select(&candidates, None).unwrap();
        assert!(selection.id.contains("alsa_output"));
        assert!(!selection.follows_default_sink());
    }

    #[test]
    fn refuses_to_capture_a_microphone_even_when_asked_by_name() {
        let error = select(&devices(), Some("Auna Mic CM900 Mono")).unwrap_err();
        let AudioError::NotAMonitor { selection } = error else {
            panic!("expected a microphone refusal");
        };
        assert_eq!(selection.kind, SourceKind::Microphone);
        assert!(selection.to_string().contains("microphone"));
    }

    #[test]
    fn an_explicit_override_may_pick_an_unclassified_device() {
        let candidates = vec![("pipewire:42".to_string(), "Some App".to_string())];
        let selection = select(&candidates, Some("pipewire:42")).unwrap();
        assert_eq!(selection.kind, SourceKind::Unknown);
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
        let candidates = vec![(
            "pipewire:alsa_input.usb-Auna_Mic_CM900-00.mono-fallback".to_string(),
            "Auna Mic CM900 Mono".to_string(),
        )];
        let error = select(&candidates, None).unwrap_err();
        assert!(error.to_string().contains("no monitor source"));
    }
}
