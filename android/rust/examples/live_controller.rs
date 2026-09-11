//! A minimal stand-in for the desktop app, used to drive a live receiver.
//!
//! Exists so a real tablet can be exercised without launching the whole Tauri
//! UI:<
//!
//! ```text
//! # 1. ask the receiver to start pairing (its screen shows a 6 digit code)
//! cargo run --example live_controller -- pair-request 192.168.1.11
//!
//! # 2. complete pairing with the code shown on the tablet
//! cargo run --example live_controller -- pair-confirm 192.168.1.11 123456
//!
//! # 3. drive the pointer / keyboard
//! cargo run --example live_controller -- move 192.168.1.11 900 1400
//! cargo run --example live_controller -- click 192.168.1.11
//! cargo run --example live_controller -- key 192.168.1.11 0x41
//! ```
//!
//! The controller identity (TLS key, cluster id, pair secret) is persisted in
//! `--state-dir` so the three invocations are the *same* peer; the receiver
//! matches a pairing confirmation against the public key that asked for it.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mykvm_core::discovery::{
    decode_discovery_packet, encode_discovery_payload, local_peer_id, peer_from_discovery_packet,
};
use mykvm_core::packet::{
    ClipboardPacket, CLIPBOARD_PROTOCOL,
    DiscoveryPacket, DiscoveryPairingFields, InputEvent, InputPacket, LanPeer, LanPeerScreen,
    MouseButton, DISCOVERY_PORT, INPUT_PROTOCOL, PROTOCOL_VERSION,
};
use mykvm_core::quic_transport;
use serde::{Deserialize, Serialize};

const CONTROLLER_ID: &str = "live-controller";
const CONTROLLER_NAME: &str = "Live Test Controller";
const CONTROLLER_HOST: &str = "live-controller-host";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!(
            "usage: live_controller <pair-request|pair-confirm|move|click|rclick|mclick|scroll|key|combo|trace|clip|status> <host> [args...]"
        );
        std::process::exit(2);
    }
    let command = args[0].as_str();
    let host = args[1].clone();
    let rest: Vec<String> = args[2..].to_vec();

    let state_dir = std::env::var("MYKVM_LIVE_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("mykvm-live-controller"));
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    let mut state = LiveState::load(&state_dir);

    let result = match command {
        "pair-request" => pair_request(&host, &state_dir, &mut state),
        "pair-confirm" => {
            let code = rest.first().cloned().unwrap_or_default();
            pair_confirm(&host, &state_dir, &state, &code)
        }

        // ------------------------------------------------------------------
        // Input
        // ------------------------------------------------------------------
        "move" => {
            let x: i32 = rest.first().and_then(|v| v.parse().ok()).unwrap_or(900);
            let y: i32 = rest.get(1).and_then(|v| v.parse().ok()).unwrap_or(1400);
            move_cursor(&host, &state_dir, &state, x, y)
        }
        "click" => click(&host, &state_dir, &state),
        "rclick" => mouse_button(&host, &state_dir, &state, MouseButton::Right),
        "mclick" => mouse_button(&host, &state_dir, &state, MouseButton::Middle),
        "scroll" => send_events(
            &host,
            &state_dir,
            &state,
            &[InputEvent::Scroll {
                delta_x: 0,
                delta_y: 1,
            }],
        ),
        "key" => {
            let vk = rest
                .first()
                .and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x41);
            send_key(&host, &state_dir, &state, vk)
        }
        "trace" => trace(&host, &state_dir, &state),
        "combo" => {
            // Two key codes: hold the first, press the second (e.g. Ctrl+V).
            let m = rest.first()
                .and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x11);
            let k = rest.get(1)
                .and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x56);
            send_combo(&host, &state_dir, &state, m, k)
        }
        "clip" => {
            let text = rest
                .first()
                .cloned()
                .unwrap_or_else(|| "hello from the desktop".into());
            send_clipboard(&host, &state_dir, &state, &text)
        }

        "status" => {
            println!("{}", state.describe());
            Ok(())
        }
        other => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    };

    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// Moves the cursor, warming the QUIC connection first.
///
/// A datagram handed to a connection that is still handshaking is dropped (QUIC
/// datagrams are unreliable by design). The desktop hides this behind
/// `warm_quic_peer`; this tool warms it by sending the move twice, which is also
/// what a real user's continuous mouse movement does.
fn move_cursor(
    host: &str,
    dir: &PathBuf,
    state: &LiveState,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let event = InputEvent::MouseMove {
        screen_id: screen_id(),
        x,
        y,
    };
    send_events(host, dir, state, &[event.clone(), event])
}

/// Press and release in one connection, 60ms apart.
/// Press and release a specific mouse button in one connection.
fn mouse_button(
    host: &str,
    dir: &PathBuf,
    state: &LiveState,
    button: MouseButton,
) -> Result<(), String> {
    send_events(
        host,
        dir,
        state,
        &[
            InputEvent::MouseButton { button, down: true },
            InputEvent::MouseButton { button, down: false },
        ],
    )
}

fn click(host: &str, dir: &PathBuf, state: &LiveState) -> Result<(), String> {
    send_events(
        host,
        dir,
        state,
        &[
            InputEvent::MouseButton {
                button: MouseButton::Left,
                down: true,
            },
            InputEvent::MouseButton {
                button: MouseButton::Left,
                down: false,
            },
        ],
    )
}

fn send_key(host: &str, dir: &PathBuf, state: &LiveState, vk: u16) -> Result<(), String> {
    send_events(
        host,
        dir,
        state,
        &[
            InputEvent::Key {
                key_code: vk,
                down: true,
            },
            InputEvent::Key {
                key_code: vk,
                down: false,
            },
        ],
    )
}

/// Holds [modifier], taps [keyCode], releases the modifier -- all in one batch.
fn send_combo(
    host: &str,
    dir: &PathBuf,
    state: &LiveState,
    modifier: u16,
    key_code: u16,
) -> Result<(), String> {
    send_events(
        host,
        dir,
        state,
        &[
            InputEvent::Key { key_code: modifier, down: true },
            InputEvent::Key { key_code, down: true },
            InputEvent::Key { key_code, down: false },
            InputEvent::Key { key_code: modifier, down: false },
        ],
    )
}

/// Sends one clipboard text payload over the reliable stream.
fn send_clipboard(
    host: &str,
    dir: &PathBuf,
    state: &LiveState,
    text: &str,
) -> Result<(), String> {
    if state.receiver_public_key.is_empty() {
        return Err("run pair-request first".into());
    }
    let (transport, _rx) = start_transport(dir)?;
    let addr = format!("{}:{}", strip_port(host), state.receiver_quic_port);
    let peer = transport.peer(
        addr.clone(),
        state.receiver_public_key.clone(),
        PROTOCOL_VERSION,
    );

    let packet = ClipboardPacket {
        protocol: CLIPBOARD_PROTOCOL.into(),
        origin_id: local_peer_id(CONTROLLER_ID),
        origin_transport_public_key: transport.public_key().to_string(),
        target_id: state.receiver_id.clone(),
        cluster_id: state.cluster_id.clone(),
        pair_secret: state.pair_secret.clone(),
        signature: String::new(),
        formats: Vec::new(),
        text: text.to_string(),
        image: None,
        sequence: 1,
    };
    let payload = rmp_serde::to_vec_named(&packet).map_err(|e| e.to_string())?;
    transport.send_stream_expect_ack(peer, payload)?;
    println!("sent clipboard ({} chars) to {addr}", text.chars().count());
    Ok(())
}

/// Sweeps the cursor across the screen: 60 moves that both warm the connection
/// and leave the pointer somewhere visible for a screenshot.
fn trace(host: &str, dir: &PathBuf, state: &LiveState) -> Result<(), String> {
    let events: Vec<InputEvent> = (0..60)
        .map(|step| {
            let t = step as f64 / 59.0;
            InputEvent::MouseMove {
                screen_id: screen_id(),
                x: (200.0 + t * 1400.0) as i32,
                y: (500.0 + t * 1900.0) as i32,
            }
        })
        .collect();
    send_events(host, dir, state, &events)
}

fn screen_id() -> String {
    "live-controller-screen-1".into()
}

// ---------------------------------------------------------------------------
// Persisted controller identity
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct LiveState {
    /// The receiver's QUIC port and pinned certificate, learned from the probe.
    receiver_quic_port: u16,
    receiver_public_key: String,
    receiver_id: String,
    /// Credentials this controller hands to the receiver during pairing.
    cluster_id: String,
    pair_secret: String,
}

impl LiveState {
    fn load(dir: &PathBuf) -> Self {
        let path = dir.join("controller.json");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: &PathBuf) {
        let path = dir.join("controller.json");
        if let Ok(raw) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, raw);
        }
    }

    fn ensure_credentials(&mut self) {
        if self.cluster_id.is_empty() {
            self.cluster_id = format!("cluster-live-{}", hex(8));
        }
        if self.pair_secret.is_empty() {
            self.pair_secret = hex(32);
        }
    }

    fn describe(&self) -> String {
        format!(
            "receiver_id={} quic_port={} public_key={} paired_credentials={}",
            self.receiver_id,
            self.receiver_quic_port,
            if self.receiver_public_key.is_empty() { "<none>" } else { "<set>" },
            !self.pair_secret.is_empty()
        )
    }
}

fn hex(bytes: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut out = String::new();
    let mut value = nanos as u64 ^ 0x9E37_79B9_7F4A_7C15;
    for _ in 0..bytes {
        value = value.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push_str(&format!("{:02x}", (value >> 33) as u8));
    }
    out
}

fn controller_peer(state: &LiveState) -> LanPeer {
    LanPeer {
        id: local_peer_id(CONTROLLER_ID),
        name: CONTROLLER_NAME.into(),
        platform: "windows".into(),
        machine_role: "server".into(),
        cluster_id: state.cluster_id.clone(),
        pairing_required: false,
        host: CONTROLLER_HOST.into(),
        // Overwritten by the receiver with the datagram source address.
        ip: "0.0.0.0".into(),
        transport_port: DISCOVERY_PORT,
        quic_port: 0,
        transport_public_key: String::new(),
        protocol_version: PROTOCOL_VERSION,
        screen_count: 1,
        input_ready: true,
        upgrading: false,
        screens: vec![LanPeerScreen {
            id: screen_id(),
            name: "Live Controller Screen".into(),
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
            scale: 1.0,
            is_primary: true,
        }],
        app_version: "live-controller".into(),
        last_seen_ms: 0,
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn discover(host: &str, state: &LiveState) -> Result<(DiscoveryPacket, SocketAddr), String> {
    let target = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{DISCOVERY_PORT}")
    };

    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_millis(1200)))
        .map_err(|e| e.to_string())?;

    let payload = encode_discovery_payload(
        "probe",
        &controller_peer(state),
        DiscoveryPairingFields::default(),
    )?;

    let local_id = local_peer_id(CONTROLLER_ID);
    for _ in 0..5 {
        socket.send_to(&payload, &target).map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 65535];
        if let Ok((length, source)) = socket.recv_from(&mut buffer) {
            if let Some(packet) = decode_discovery_packet(&buffer[..length]) {
                if peer_from_discovery_packet(packet.clone(), source.ip().to_string(), &local_id)
                    .is_some()
                {
                    return Ok((packet, source));
                }
            }
        }
    }
    Err(format!("no reply from {target}"))
}

fn pair_request(host: &str, dir: &PathBuf, state: &mut LiveState) -> Result<(), String> {
    state.ensure_credentials();

    let target = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{DISCOVERY_PORT}")
    };
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_millis(1200)))
        .map_err(|e| e.to_string())?;

    let payload = encode_discovery_payload(
        "pair-request",
        &controller_peer(state),
        DiscoveryPairingFields::default(),
    )?;

    let local_id = local_peer_id(CONTROLLER_ID);
    for attempt in 1..=5 {
        socket.send_to(&payload, &target).map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 65535];
        if let Ok((length, source)) = socket.recv_from(&mut buffer) {
            if let Some(packet) = decode_discovery_packet(&buffer[..length]) {
                let Some(incoming) =
                    peer_from_discovery_packet(packet.clone(), source.ip().to_string(), &local_id)
                else {
                    continue;
                };
                if incoming.kind == "pair-challenge" {
                    state.receiver_quic_port = incoming.peer.quic_port;
                    state.receiver_public_key = incoming.peer.transport_public_key.clone();
                    state.receiver_id = incoming.peer.id.clone();
                    state.save(dir);
                    println!(
                        "pair-challenge from {} (attempt {attempt}): receiver={} quic_port={} key={}",
                        source,
                        incoming.peer.name,
                        incoming.peer.quic_port,
                        if incoming.peer.transport_public_key.is_empty() { "<none>" } else { "<set>" }
                    );
                    println!("now read the 6 digit code off the tablet and run:");
                    println!("  live_controller pair-confirm {host} <code>");
                    return Ok(());
                }
                println!("ignored reply kind={}", incoming.kind);
            }
        }
    }
    Err(format!("no pair-challenge from {target}"))
}

/// Sends the encrypted confirmation over QUIC and waits for the receiver's ack.
fn pair_confirm(host: &str, dir: &PathBuf, state: &LiveState, code: &str) -> Result<(), String> {
    if code.trim().is_empty() {
        return Err("pair-confirm needs the code shown on the receiver".into());
    }
    if state.receiver_public_key.is_empty() || state.receiver_quic_port == 0 {
        return Err("run pair-request first (no receiver certificate cached)".into());
    }

    let (transport, _rx) = start_transport(dir)?;
    let addr = format!("{}:{}", strip_port(host), state.receiver_quic_port);
    let peer = transport.peer(
        addr.clone(),
        state.receiver_public_key.clone(),
        PROTOCOL_VERSION,
    );

    let payload = encode_discovery_payload(
        "pair-confirm",
        &controller_peer(state),
        DiscoveryPairingFields {
            code: Some(code.trim().into()),
            cluster_id: Some(state.cluster_id.clone()),
            secret: Some(state.pair_secret.clone()),
            error: None,
        },
    )?;

    transport.send_stream_expect_ack(peer, payload)?;
    println!("pair-confirm acknowledged by {addr}");
    println!("cluster_id={} pair_secret=<set>", state.cluster_id);

    // The receiver should now advertise itself as an input target.
    match discover(host, state) {
        Ok((packet, _)) => {
            println!(
                "post-pairing state: pairing_required={} input_ready={} cluster_id={}",
                packet.peer.pairing_required,
                packet.peer.input_ready,
                if packet.peer.cluster_id.is_empty() { "<empty>" } else { "<set>" }
            );
            if packet.peer.pairing_required {
                return Err("the receiver is still advertising pairing_required".into());
            }
        }
        Err(error) => println!("warning: post-pairing probe failed: {error}"),
    }
    Ok(())
}

/// Sends a batch of events over one QUIC connection.
///
/// The connection is established lazily on the first datagram, and QUIC
/// datagrams are unreliable by design -- a datagram handed to a connection
/// that is still handshaking is simply dropped. That is fine for a mouse
/// move (the next one supersedes it) but it means a single-event test needs
/// to send something first.
fn send_events(
    host: &str,
    dir: &PathBuf,
    state: &LiveState,
    events: &[InputEvent],
) -> Result<(), String> {
    if state.receiver_public_key.is_empty() {
        return Err("run pair-request first".into());
    }
    let (transport, _rx) = start_transport(dir)?;
    let addr = format!("{}:{}", strip_port(host), state.receiver_quic_port);
    let peer = transport.peer(
        addr.clone(),
        state.receiver_public_key.clone(),
        PROTOCOL_VERSION,
    );

    // Warm the connection the way the desktop does (see `warm_quic_peer`):
    // an empty datagram establishes the QUIC handshake and is discarded by
    // the receiver. Without this the first real event is dropped while the
    // handshake is still in flight -- which for a 2-event click or keystroke
    // means losing the press and leaving an orphan release.
    let _ = transport.send_datagram(peer.clone(), Vec::new());
    std::thread::sleep(Duration::from_millis(250));

    for (index, event) in events.iter().enumerate() {
        let packet = InputPacket {
            protocol: INPUT_PROTOCOL.into(),
            target_device_id: state.receiver_id.clone(),
            origin_device_id: local_peer_id(CONTROLLER_ID),
            origin_port: transport.port(),
            origin_transport_public_key: transport.public_key().to_string(),
            origin_protocol_version: PROTOCOL_VERSION,
            cluster_id: state.cluster_id.clone(),
            // Credentials ride on every packet here; the desktop trims
            // them from the steady-state path, which this tool does not need.
            pair_secret: state.pair_secret.clone(),
            event: event.clone(),
        };
        let payload = rmp_serde::to_vec_named(&packet).map_err(|e| e.to_string())?;
        transport.send_datagram(peer.clone(), payload)?;
        if index == 0 {
            println!("sent {} event(s) to {addr}", events.len());
        }
        if events.len() > 1 {
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    // Let the datagrams leave before the transport is dropped.
    std::thread::sleep(Duration::from_millis(if events.len() > 1 { 400 } else { 600 }));
    Ok(())
}

fn start_transport(
    dir: &PathBuf,
) -> Result<
    (
        quic_transport::TransportHandle,
        Arc<Mutex<Vec<Vec<u8>>>>,
    ),
    String,
> {
    let received: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);

    // Built inline: the handler type aliases in quic_transport are private
    // (that file is a verbatim copy of the desktop module).
    let on_datagram = Arc::new(move |payload: Vec<u8>, _source: SocketAddr| {
        if let Ok(mut guard) = sink.lock() {
            guard.push(payload);
        }
    });
    let on_stream = Arc::new(|_payload: Vec<u8>, _source: SocketAddr| false);

    let transport = quic_transport::start(0, dir.clone(), on_datagram, on_stream)?;
    Ok((transport, received))
}

fn strip_port(host: &str) -> String {
    host.split(':').next().unwrap_or(host).to_string()
}
