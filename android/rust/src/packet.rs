//! Wire format shared with the MyKVM desktop app.
//!
//! Every struct here is a byte-for-byte mirror of the desktop definitions
//! (`src-tauri/src/lib.rs` and `src-tauri/src/input.rs`). The serde attributes
//! are intentionally copied verbatim: MessagePack encodes field *names*, so a
//! renamed field silently decodes as a default instead of erroring.

use serde::{Deserialize, Serialize};

pub const DISCOVERY_PROTOCOL: &str = "mykvm.discovery.v1";
pub const INPUT_PROTOCOL: &str = "mykvm.input.v1";
pub const DISCOVERY_PORT: u16 = 47833;

/// Version of the input/transport plane. Mirrors `quic_transport::PROTOCOL_VERSION`.
pub use crate::quic_transport::PROTOCOL_VERSION;

pub fn default_transport_port() -> u16 {
    DISCOVERY_PORT
}

pub fn default_protocol_version() -> u16 {
    PROTOCOL_VERSION
}

// ---------------------------------------------------------------------------
// Input plane
// ---------------------------------------------------------------------------

/// Mirrors `shared_input::MouseButton`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Mirrors `shared_input::InputEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum InputEvent {
    MouseMove { screen_id: String, x: i32, y: i32 },
    MouseButton { button: MouseButton, down: bool },
    Scroll { delta_x: i32, delta_y: i32 },
    Key { key_code: u16, down: bool },
}

/// Mirrors `input::InputPacket`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputPacket {
    pub protocol: String,
    #[serde(default)]
    pub target_device_id: String,
    #[serde(default)]
    pub origin_device_id: String,
    #[serde(default)]
    pub origin_port: u16,
    #[serde(default)]
    pub origin_transport_public_key: String,
    #[serde(default = "default_protocol_version")]
    pub origin_protocol_version: u16,
    #[serde(default)]
    pub cluster_id: String,
    #[serde(default)]
    pub pair_secret: String,
    pub event: InputEvent,
}

// ---------------------------------------------------------------------------
// Clipboard plane
// ---------------------------------------------------------------------------

pub const CLIPBOARD_PROTOCOL: &str = "mykvm.clipboard.v1";

/// Mirrors `clipboard::ClipboardImage`. RGBA, base64, row-major.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub rgba_base64: String,
}

/// Mirrors `lib::ClipboardFormat`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFormat {
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image: Option<ClipboardImage>,
}

/// Mirrors `lib::ClipboardPacket`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPacket {
    pub protocol: String,
    pub origin_id: String,
    #[serde(default)]
    pub origin_transport_public_key: String,
    #[serde(default)]
    pub target_id: String,
    #[serde(default)]
    pub cluster_id: String,
    #[serde(default)]
    pub pair_secret: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub formats: Vec<ClipboardFormat>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image: Option<ClipboardImage>,
    pub sequence: u64,
}

// ---------------------------------------------------------------------------
// Discovery plane
// ---------------------------------------------------------------------------

/// Mirrors `lib::LanPeerScreen`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPeerScreen {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub is_primary: bool,
}

/// Mirrors `lib::LanPeer`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPeer {
    pub id: String,
    pub name: String,
    pub platform: String,
    #[serde(default)]
    pub machine_role: String,
    #[serde(default)]
    pub cluster_id: String,
    #[serde(default)]
    pub pairing_required: bool,
    pub host: String,
    pub ip: String,
    #[serde(default = "default_transport_port")]
    pub transport_port: u16,
    #[serde(default)]
    pub quic_port: u16,
    #[serde(default)]
    pub transport_public_key: String,
    #[serde(default = "default_protocol_version")]
    pub protocol_version: u16,
    pub screen_count: usize,
    #[serde(default)]
    pub input_ready: bool,
    #[serde(default)]
    pub upgrading: bool,
    #[serde(default)]
    pub screens: Vec<LanPeerScreen>,
    pub app_version: String,
    pub last_seen_ms: u64,
}

/// Mirrors `lib::DiscoveryPacket`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPacket {
    pub protocol: String,
    pub kind: String,
    pub peer: LanPeer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_cluster_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_error: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct DiscoveryPairingFields {
    pub code: Option<String>,
    pub cluster_id: Option<String>,
    pub secret: Option<String>,
    pub error: Option<String>,
}

/// Mirrors `lib::IncomingDiscovery`.
///
/// The pairing fields are populated but unread on Android: a receiver only ever
/// takes its pairing credentials from the encrypted QUIC `pair-confirm` stream,
/// so the plaintext copies here are deliberately ignored.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct IncomingDiscovery {
    pub kind: String,
    pub peer: LanPeer,
    pub pairing_code: Option<String>,
    pub pair_cluster_id: Option<String>,
    pub pair_secret: Option<String>,
}

// ---------------------------------------------------------------------------
// Persisted receiver state
// ---------------------------------------------------------------------------

/// Mirrors `lib::PairedController`. The desktop uses this to decide whether an
/// incoming packet is allowed to inject, so the field set must stay in sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedController {
    pub id: String,
    pub name: String,
    pub host: String,
    pub ip: String,
    pub transport_public_key: String,
    #[serde(default = "default_protocol_version")]
    pub protocol_version: u16,
    pub cluster_id: String,
    pub paired_at_ms: u64,
}

/// The receiver's own screen, reported to the desktop in discovery announces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub id: String,
    pub device_id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub is_primary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of duplicating these structs: a packet encoded by the
    /// desktop must decode here. This pins the MessagePack field names.
    #[test]
    fn input_packet_matches_desktop_field_names() {
        let json = r#"{
            "protocol": "mykvm.input.v1",
            "targetDeviceId": "peer-pixel8-abc",
            "originDeviceId": "",
            "originPort": 47834,
            "originTransportPublicKey": "",
            "originProtocolVersion": 1,
            "clusterId": "cluster-abc",
            "pairSecret": "",
            "event": { "type": "mouseMove", "screen_id": "s1", "x": 10, "y": 20 }
        }"#;

        let event: InputEvent =
            serde_json::from_str(r#"{"type":"mouseMove","screen_id":"s1","x":10,"y":20}"#).unwrap();
        assert_eq!(
            event,
            InputEvent::MouseMove {
                screen_id: "s1".into(),
                x: 10,
                y: 20
            }
        );

        let packet: InputPacket = serde_json::from_str(json).unwrap();
        assert_eq!(packet.target_device_id, "peer-pixel8-abc");
        assert_eq!(packet.protocol, INPUT_PROTOCOL);
    }

    #[test]
    fn input_event_key_and_scroll_round_trip() {
        for event in [
            InputEvent::Key {
                key_code: 0x41,
                down: true,
            },
            InputEvent::Scroll {
                delta_x: 0,
                delta_y: -1,
            },
            InputEvent::MouseButton {
                button: MouseButton::Left,
                down: false,
            },
        ] {
            let bytes = rmp_serde::to_vec_named(&event).unwrap();
            let decoded: InputEvent = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(decoded, event);
        }
    }
}
