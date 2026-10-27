//! The transport seam.
//!
//! Everything above this module talks to a [`Link`], never to a USB port, so the
//! frame budget, session restore order and reply parsing are all testable
//! against an in-memory device. The only implementation that touches hardware is
//! [`DygmaLink`].
//!
//! # Why this does not use `dygma_focus` for I/O
//!
//! The protocol acknowledges *every* command with `\r\n.\r\n`, including each
//! `led.theme` frame. At the measured ~70 frames per second that is over a
//! kilobyte per second of acknowledgements nobody asked for. The firmware writes
//! them into the same USB endpoint it uses for replies, so they have to be
//! discarded periodically or the kernel buffer fills and the device stalls.
//!
//! `dygma_focus::Focus` hard-codes a five-second read timeout and keeps its port
//! private, so it cannot peek or discard. It remains the source of the device
//! table used to enumerate ports; the I/O is done here against `serialport`
//! directly so a non-blocking drain is possible.
//!
//! Its `LedMode` vocabulary is deliberately *not* used: it names modes 4–6 as
//! Red/Green/Blue, while firmware v1.0.12-beta rejects anything above 3.

use std::fmt;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use serialport::SerialPort;

/// How long to wait for a reply before calling the port dead.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for the first byte of a drain, which is only ever used to
/// collect bytes that are already buffered.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(1);

/// The verifier the firmware ends every reply with.
const TERMINATOR: &[u8] = b"\r\n.\r\n";

/// A failure talking to the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// No supported Dygma device could be found.
    NotFound { looked_for: String },
    /// The operating system refused or lost the port.
    Io(String),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::NotFound { looked_for } => write!(
                f,
                "no Dygma keyboard found (looked for {looked_for} in USB devices and /dev/serial/by-id)"
            ),
            LinkError::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for LinkError {}

/// The commands the daemon needs from a keyboard.
pub trait Link {
    /// Sends a command without waiting for the device to answer.
    fn send(&mut self, command: &str) -> Result<(), LinkError>;

    /// Sends a command and returns the device's reply, without its terminator.
    fn query(&mut self, command: &str) -> Result<String, LinkError>;

    /// Discards acknowledgements the device has already written, returning how
    /// many replies were thrown away.
    ///
    /// Defaults to doing nothing so fakes need not implement it.
    fn drain(&mut self) -> Result<usize, LinkError> {
        Ok(0)
    }
}

/// The wired Raise's USB identity.
const RAISE_KEYBOARD: (u16, u16) = (0x1209, 0x2201);

/// The same board in bootloader mode. It answers no Focus commands, so it must
/// never be selected — writing LED frames to it just times out after five
/// seconds per command.
const RAISE_BOOTLOADER: (u16, u16) = (0x1209, 0x2200);

/// Appends the newline the Focus protocol uses to terminate a command.
fn frame_command(command: &str) -> String {
    let mut framed = String::with_capacity(command.len() + 1);
    framed.push_str(command);
    framed.push('\n');
    framed
}

/// Counts complete replies in an accumulated byte buffer.
fn count_replies(buffer: &[u8]) -> usize {
    buffer
        .windows(TERMINATOR.len())
        .filter(|window| *window == TERMINATOR)
        .count()
}

/// Removes the first complete reply from the front of the buffer and returns its
/// body, without the terminator or surrounding whitespace.
///
/// Returns `None` while the buffer holds no complete reply. An empty body means
/// the reply was a bare acknowledgement.
fn take_first_reply(buffer: &mut Vec<u8>) -> Option<String> {
    let end = buffer
        .windows(TERMINATOR.len())
        .position(|window| window == TERMINATOR)?;
    let body = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
    buffer.drain(..end + TERMINATOR.len());
    Some(body)
}

/// A link backed by a real port.
pub struct DygmaLink {
    port: Box<dyn SerialPort>,
    port_name: String,
}

impl DygmaLink {
    /// Opens a specific port, skipping discovery.
    pub fn from_port(port_name: &str) -> Result<Self, LinkError> {
        let port = serialport::new(port_name, 115_200)
            .data_bits(serialport::DataBits::Eight)
            .flow_control(serialport::FlowControl::None)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .timeout(REPLY_TIMEOUT)
            .open()
            .map_err(|error| LinkError::Io(format!("cannot open {port_name}: {error}")))?;
        Ok(Self {
            port,
            port_name: port_name.to_string(),
        })
    }

    /// Discovers a wired Raise by USB identity and opens it.
    ///
    /// Deliberately no "first serial port on the system" fallback: writing LED
    /// frames into an unrelated device is worse than failing to find one.
    pub fn discover() -> Result<Self, LinkError> {
        if let Some(port) = find_by_usb_identity() {
            return Self::from_port(&port);
        }
        if let Some(port) = find_by_serial_symlink() {
            return Self::from_port(&port);
        }
        let (vid, pid) = RAISE_KEYBOARD;
        Err(LinkError::NotFound {
            looked_for: format!(
                "{vid:04x}:{pid:04x} (bootloader {:04x}:{:04x} excluded)",
                RAISE_BOOTLOADER.0, RAISE_BOOTLOADER.1
            ),
        })
    }

    /// The port this link is talking to, for logging.
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    fn set_timeout(&mut self, timeout: Duration) -> Result<(), LinkError> {
        self.port
            .set_timeout(timeout)
            .map_err(|error| LinkError::Io(error.to_string()))
    }

    /// Reads whatever is already buffered, without waiting for anything new.
    fn read_available(&mut self, out: &mut Vec<u8>) -> Result<(), LinkError> {
        self.set_timeout(DRAIN_TIMEOUT)?;
        let mut chunk = [0u8; 4096];
        loop {
            match self.port.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => out.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(LinkError::Io(error.to_string())),
            }
        }
        Ok(())
    }
}

/// Enumerates Dygma devices through `dygma_focus` and keeps the wired Raise.
///
/// The crate's table also lists the Defy and the Raise 2, so taking the first
/// entry would aim frames — and a persisted `led.mode` change — at whichever
/// keyboard enumerated first. The identity must match exactly.
fn find_by_usb_identity() -> Option<String> {
    let devices = dygma_focus::prelude::Focus::find_all_devices().ok()?;
    devices
        .into_iter()
        .filter(|device| {
            (
                device.hardware.usb.vendor_id,
                device.hardware.usb.product_id,
            ) != RAISE_BOOTLOADER
        })
        .find(|device| {
            (
                device.hardware.usb.vendor_id,
                device.hardware.usb.product_id,
            ) == RAISE_KEYBOARD
        })
        .map(|device| device.serial_port)
}

/// Reads a port's USB vendor and product IDs from sysfs.
///
/// Used by the symlink fallback, where the filename carries vendor and product
/// *names* rather than IDs, so a name match cannot distinguish a Raise from a
/// Raise 2.
fn usb_ids_for_port(port: &str) -> Option<(u16, u16)> {
    let resolved = std::fs::canonicalize(port).ok()?;
    let tty = resolved.file_name()?.to_string_lossy().into_owned();
    let mut node = std::fs::canonicalize(format!("/sys/class/tty/{tty}/device")).ok()?;
    loop {
        let vendor = std::fs::read_to_string(node.join("idVendor")).ok();
        let product = std::fs::read_to_string(node.join("idProduct")).ok();
        if let (Some(vendor), Some(product)) = (vendor, product) {
            return Some((
                u16::from_str_radix(vendor.trim(), 16).ok()?,
                u16::from_str_radix(product.trim(), 16).ok()?,
            ));
        }
        node = node.parent()?.to_path_buf();
    }
}

/// Falls back to the stable `/dev/serial/by-id` symlinks, which survive
/// `/dev/ttyACM*` renumbering when the port enumerator cannot report USB
/// metadata. Each candidate is verified against its sysfs IDs, never its name.
fn find_by_serial_symlink() -> Option<String> {
    let entries = std::fs::read_dir("/dev/serial/by-id").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let candidate = path.to_string_lossy().into_owned();
        if usb_ids_for_port(&candidate) == Some(RAISE_KEYBOARD) {
            return Some(candidate);
        }
    }
    None
}

impl Link for DygmaLink {
    fn send(&mut self, command: &str) -> Result<(), LinkError> {
        let framed = frame_command(command);
        self.port
            .write_all(framed.as_bytes())
            .map_err(|error| LinkError::Io(error.to_string()))?;
        // Waits only for the bytes to leave this machine; the device's reply is
        // deliberately not read, which is what makes ~70 fps possible.
        self.port
            .flush()
            .map_err(|error| LinkError::Io(error.to_string()))
    }

    fn query(&mut self, command: &str) -> Result<String, LinkError> {
        // Anything already buffered is an acknowledgement for an earlier frame,
        // not the answer to this command.
        self.drain()?;
        self.set_timeout(REPLY_TIMEOUT)?;
        self.send(command)?;

        let mut buffer = Vec::with_capacity(2048);
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut chunk = [0u8; 4096];
        loop {
            match self.port.read(&mut chunk) {
                // A port returning no bytes must not spin past the deadline.
                Ok(0) => std::thread::sleep(Duration::from_millis(1)),
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(LinkError::Io(error.to_string())),
            }
            // The board acks every command, so the first reply to arrive may be
            // an acknowledgement for a frame sent earlier. Only a reply with a
            // body can be the answer to a question.
            while let Some(body) = take_first_reply(&mut buffer) {
                if !body.is_empty() {
                    return Ok(body);
                }
            }
            if Instant::now() > deadline {
                return Err(LinkError::Io(format!(
                    "timed out waiting for a reply to {command:?}"
                )));
            }
        }
    }

    fn drain(&mut self) -> Result<usize, LinkError> {
        let mut buffer = Vec::new();
        self.read_available(&mut buffer)?;
        self.set_timeout(REPLY_TIMEOUT)?;
        Ok(count_replies(&buffer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_a_command_with_a_single_newline() {
        assert_eq!(frame_command("led.mode 0"), "led.mode 0\n");
    }

    #[test]
    fn never_double_terminates() {
        assert!(!frame_command("led.theme").ends_with("\n\n"));
    }

    #[test]
    fn counts_acks_and_payloads_alike() {
        assert_eq!(count_replies(b"\r\n.\r\n"), 1);
        assert_eq!(count_replies(b"\r\n.\r\n\r\n.\r\n\r\n.\r\n"), 3);
        assert_eq!(count_replies(b"v1.0.12\r\n.\r\n"), 1);
        assert_eq!(count_replies(b"partial"), 0);
    }

    #[test]
    fn strips_the_terminator_from_a_reply() {
        let mut buffer = b"v1.0.12-beta\r\n.\r\n".to_vec();
        assert_eq!(
            take_first_reply(&mut buffer).as_deref(),
            Some("v1.0.12-beta")
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn an_acknowledgement_has_an_empty_body() {
        let mut buffer = b"\r\n.\r\n".to_vec();
        assert_eq!(take_first_reply(&mut buffer).as_deref(), Some(""));
    }

    #[test]
    fn skips_past_a_stale_acknowledgement_to_the_real_reply() {
        // A frame's ack arrived before the answer to the command we asked.
        let mut buffer = b"\r\n.\r\nv1.0.12-beta\r\n.\r\n".to_vec();
        assert_eq!(take_first_reply(&mut buffer).as_deref(), Some(""));
        assert_eq!(
            take_first_reply(&mut buffer).as_deref(),
            Some("v1.0.12-beta")
        );
        assert_eq!(take_first_reply(&mut buffer), None);
    }

    #[test]
    fn waits_for_a_partial_reply_to_complete() {
        let mut buffer = b"v1.0.1".to_vec();
        assert_eq!(take_first_reply(&mut buffer), None);
        buffer.extend_from_slice(b"2\r\n.\r\n");
        assert_eq!(take_first_reply(&mut buffer).as_deref(), Some("v1.0.12"));
    }

    #[test]
    fn not_found_names_the_identifiers_that_were_sought() {
        let error = LinkError::NotFound {
            looked_for: "1209:2201".to_string(),
        };
        assert!(error.to_string().contains("1209:2201"));
    }
}
