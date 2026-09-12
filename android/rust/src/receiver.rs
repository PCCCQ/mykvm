//! Wires the QUIC input plane to the UDP discovery plane.
//!
//! The receiver is the QUIC *server* side: the desktop dials in and pushes
//! input datagrams. The same QUIC endpoint also accepts the encrypted
//! `pair-confirm` stream, which is why pairing completion lives here rather
//! than in the discovery loop.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver as MpscReceiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::discovery::{spawn_discovery, DiscoveryHandle, ReceiverConfig, SharedControllerEndpoint};
use crate::events::{EventSink, ReceiverEvent};
use crate::packet::{
    ClipboardPacket, DiagnosticsPacket, InputPacket, Screen, CLIPBOARD_PROTOCOL,
    DIAGNOSTICS_PROTOCOL, INPUT_PROTOCOL,
};
use crate::pairing::{complete_pairing_from_confirm, SharedChallenge};
use crate::state::{input_authorized, now_ms, OriginCache, ReceiverLayout};
use crate::quic_transport;

pub struct ReceiverStartup {
    pub device_name: String,
    pub stable_id: String,
    pub app_version: String,
    pub screen_id: String,
    pub screen_width: i32,
    pub screen_height: i32,
    pub discovery_port: u16,
    pub quic_port: u16,
    /// App-private directory used to persist the TLS identity and layout.
    pub identity_dir: PathBuf,
    pub initial_layout: ReceiverLayout,
}

pub struct Receiver {
    pub config: ReceiverConfig,
    pub layout: Arc<Mutex<ReceiverLayout>>,
    transport: quic_transport::TransportHandle,
    discovery: Option<DiscoveryHandle>,
    events_rx: Option<MpscReceiver<ReceiverEvent>>,
    /// Where the paired desktop was last seen, for the diagnostics upload.
    controller_endpoint: SharedControllerEndpoint,
    /// Guards `stop()` so the explicit call in `nativeStop` and the `Drop` that
    /// follows it cannot shut the endpoint down (and log) twice.
    stopped: bool,
}

impl Receiver {
    /// Starts the QUIC endpoint first: it may drift off the preferred port, and
    /// discovery must advertise the port that was actually bound.
    pub fn start(startup: ReceiverStartup) -> Result<Self, String> {
        let layout = Arc::new(Mutex::new(startup.initial_layout));
        let challenge: SharedChallenge = Arc::new(Mutex::new(None));
        let (events_tx, events_rx) = mpsc::channel::<ReceiverEvent>();
        let sink_tx = events_tx.clone();

        let on_datagram_layout = Arc::clone(&layout);
        let on_datagram_events: EventSink =
            Arc::new(move |event| {
                let _ = sink_tx.send(event);
            });
        let origin_cache = Arc::new(Mutex::new(OriginCache::default()));

        let on_datagram = {
            let layout = Arc::clone(&on_datagram_layout);
            let events = Arc::clone(&on_datagram_events);
            let origin_cache = Arc::clone(&origin_cache);
            Arc::new(move |payload: Vec<u8>, source: SocketAddr| {
                handle_input_datagram(&layout, &origin_cache, &events, &payload, source);
            })
        };

        let on_stream = {
            let layout = Arc::clone(&layout);
            let challenge = Arc::clone(&challenge);
            let events = Arc::clone(&on_datagram_events);
            let events_tx = events_tx.clone();
            Arc::new(move |payload: Vec<u8>, _source: SocketAddr| -> bool {
                handle_stream_packet(&layout, &challenge, &events, &events_tx, &payload)
            })
        };

        let transport =
            quic_transport::start(startup.quic_port, startup.identity_dir, on_datagram, on_stream)?;

        // The screen lives behind the config so a rotation or a PC-mode window
        // resize can re-advertise a new size *without* tearing the QUIC endpoint
        // down. Restarting used to drop every live connection, which the desktop
        // then logged as "the server refused to accept a new connection".
        let screen = Arc::new(Mutex::new(Screen {
            id: startup.screen_id.clone(),
            device_id: startup.stable_id.clone(),
            name: startup.device_name.clone(),
            x: 0,
            y: 0,
            width: startup.screen_width,
            height: startup.screen_height,
            // 1.0 so the coordinates the desktop sends are already device
            // pixels and the injector needs no scaling.
            scale: 1.0,
            is_primary: true,
        }));

        let config = ReceiverConfig {
            device_name: startup.device_name,
            stable_id: startup.stable_id,
            app_version: startup.app_version,
            screen_id: startup.screen_id,
            screen,
            discovery_port: startup.discovery_port,
            // Advertise what we actually bound, not what we asked for.
            quic_port: transport.port(),
            transport_public_key: transport.public_key().to_string(),
            protocol_version: quic_transport::PROTOCOL_VERSION,
        };

        let events: EventSink = {
            let tx = events_tx.clone();
            Arc::new(move |event| {
                let _ = tx.send(event);
            })
        };

        let controller_endpoint: SharedControllerEndpoint = Arc::new(Mutex::new(None));
        let discovery = spawn_discovery(
            config.clone(),
            Arc::clone(&layout),
            Arc::clone(&challenge),
            Arc::clone(&controller_endpoint),
            events,
        )?;

        log::info!(
            "receiver started: {} ({}) quic={} discovery={}",
            config.device_name,
            config.stable_id,
            config.quic_port,
            config.discovery_port
        );

        Ok(Self {
            config,
            layout,
            transport,
            discovery: Some(discovery),
            events_rx: Some(events_rx),
            controller_endpoint,
            stopped: false,
        })
    }

    /// Re-advertises a new screen size in place (rotation, PC-mode resize).
    pub fn set_screen_size(&self, width: i32, height: i32) {
        self.config.set_screen_size(width, height);
    }

    /// Pushes this device's diagnostics log to the paired desktop.
    ///
    /// One-shot and best effort: the desktop writes it next to its own log so a
    /// tester gets the phone's side of the story without a cable.
    pub fn send_diagnostics(&self, file_name: &str, text: &str) -> Result<(), String> {
        let endpoint = self
            .controller_endpoint
            .lock()
            .map_err(|_| "controller endpoint lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "还没有看到已配对的电脑，请先让电脑连接一次".to_string())?;
        let layout = self.layout_snapshot();
        if !layout.is_paired() {
            return Err("尚未配对，无法发送日志".into());
        }

        let packet = DiagnosticsPacket {
            protocol: DIAGNOSTICS_PROTOCOL.to_string(),
            // The derived peer id, i.e. the same id the desktop knows us by.
            origin_id: crate::discovery::local_peer_id(&self.config.stable_id),
            origin_transport_public_key: self.config.transport_public_key.clone(),
            cluster_id: layout.cluster_id.clone(),
            pair_secret: layout.pair_secret.clone(),
            file_name: file_name.to_string(),
            text: text.to_string(),
        };
        let payload = rmp_serde::to_vec_named(&packet)
            .map_err(|error| format!("failed to encode diagnostics: {error}"))?;

        let peer = self.transport.peer(
            format!("{}:{}", endpoint.ip, endpoint.quic_port),
            endpoint.transport_public_key.clone(),
            endpoint.protocol_version,
        );
        self.transport.send_stream_expect_ack(peer, payload)
    }

    pub fn layout_snapshot(&self) -> ReceiverLayout {
        self.layout
            .lock()
            .map(|layout| layout.clone())
            .unwrap_or_else(|_| ReceiverLayout::new_unpaired())
    }

    /// Replace the persisted layout (used when the app restores a pairing on
    /// the next launch).
    pub fn set_layout(&self, layout: ReceiverLayout) {
        if let Ok(mut guard) = self.layout.lock() {
            *guard = layout;
        }
    }

    pub fn is_paired(&self) -> bool {
        self.layout_snapshot().is_paired()
    }

    /// Hands the event stream to the caller (the JNI poll loop). After this
    /// the receiver no longer consumes events itself.
    pub fn take_events(&mut self) -> Option<MpscReceiver<ReceiverEvent>> {
        self.events_rx.take()
    }

    /// Idempotent: `Drop` runs right after the explicit stop from `nativeStop`,
    /// and the second call used to log "receiver stopped" a second time (which
    /// made a single configuration restart look like two shutdowns in the log).
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(mut discovery) = self.discovery.take() {
            discovery.stop();
        }
        self.transport.shutdown();
        log::info!("receiver stopped");
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Mirrors `input::handle_input_datagram`, minus the clipboard/file side effects
/// the Android receiver does not implement.
/// Counters so the input path is observable from logcat without logging
/// every mouse move (which would itself cost more than the injection).
static ACCEPTED_PACKETS: AtomicU64 = AtomicU64::new(0);
static REJECTED_PACKETS: AtomicU64 = AtomicU64::new(0);

fn handle_input_datagram(
    layout: &Arc<Mutex<ReceiverLayout>>,
    origin_cache: &Arc<Mutex<OriginCache>>,
    events: &EventSink,
    payload: &[u8],
    source: SocketAddr,
) {
    let Ok(packet) = rmp_serde::from_slice::<InputPacket>(payload) else {
        return;
    };
    if packet.protocol != INPUT_PROTOCOL {
        return;
    }

    // Steady-state datagrams omit the ~0.5KB pairing block; those are admitted
    // only while this exact source address still has a fresh authorization from
    // a credentialled packet.
    let carries_credentials = !packet.pair_secret.trim().is_empty();
    let is_first_accepted = ACCEPTED_PACKETS.load(Ordering::Relaxed) == 0;

    let authorized = {
        let Ok(layout) = layout.lock() else {
            return;
        };
        if carries_credentials {
            if !input_authorized(
                &layout,
                &packet.cluster_id,
                &packet.pair_secret,
                &packet.origin_transport_public_key,
                &packet.origin_device_id,
            ) {
                let rejected = REJECTED_PACKETS.fetch_add(1, Ordering::Relaxed);
                // Throttled: a misconfigured controller would otherwise flood.
                if rejected < 5 || rejected % 500 == 0 {
                    log::warn!(
                        "input rejected ({} total): cluster={} origin={} key={} paired_controllers={}",
                        rejected + 1,
                        if packet.cluster_id.is_empty() { "<empty>" } else { "<set>" },
                        if packet.origin_device_id.is_empty() { "<empty>" } else { "<set>" },
                        if packet.origin_transport_public_key.is_empty() { "<empty>" } else { "<set>" },
                        layout.paired_controllers.len(),
                    );
                }
                return;
            }
            true
        } else {
            origin_cache
                .lock()
                .map(|cache| cache.is_fresh(source))
                .unwrap_or(false)
        }
    };
    if !authorized {
        let rejected = REJECTED_PACKETS.fetch_add(1, Ordering::Relaxed);
        if rejected < 5 {
            log::warn!("input discarded (no fresh authorization) from {source}");
        }
        return;
    }

    if carries_credentials {
        if let Ok(mut cache) = origin_cache.lock() {
            cache.remember(source);
        }
    }

    // Coordinates arrive already normalised to physical pixels because the
    // screen is advertised with scale 1.0, so there is nothing to map here.
    let accepted = ACCEPTED_PACKETS.fetch_add(1, Ordering::Relaxed);
    if is_first_accepted {
        log::info!("first input packet accepted from {source}");
    } else if accepted % 500 == 0 {
        log::info!("input packets accepted: {}", accepted + 1);
    }
    events(ReceiverEvent::Input(packet.event));
}

/// Handles the encrypted `pair-confirm` stream.
///
/// Ordering matters: the layout is updated *before* returning `true`, because
/// returning `true` is what releases the stream ack. The desktop waits for that
/// ack and then immediately probes for the peer; if we still advertised
/// `pairing_required: true` it would report "pairing was not accepted".
/// Entry point for every QUIC stream the desktop opens.
///
/// Two kinds arrive: the encrypted `pair-confirm`, and clipboard updates.
/// They are told apart by their protocol tag rather than by trying to
/// decode twice, so a malformed packet cannot be mistaken for the other.
fn handle_stream_packet(
    layout: &Arc<Mutex<ReceiverLayout>>,
    challenge: &SharedChallenge,
    events: &EventSink,
    events_tx: &Sender<ReceiverEvent>,
    payload: &[u8],
) -> bool {
    if is_pair_confirm(payload) {
        return handle_pairing_stream(layout, challenge, events, events_tx, payload);
    }
    handle_clipboard_stream(layout, events, payload)
}

/// Cheap protocol probe so clipboard payloads are not fed to the pairing decoder.
fn is_pair_confirm(payload: &[u8]) -> bool {
    rmp_serde::from_slice::<crate::packet::DiscoveryPacket>(payload)
        .map(|packet| packet.protocol == crate::packet::DISCOVERY_PROTOCOL)
        .unwrap_or(false)
}

/// Writes clipboard text received from a paired controller.
///
/// Images are not handled yet: the payload is up to 32MB of base64 RGBA and
/// would have to cross the JNI boundary, so image packets are logged and
/// dropped rather than half-implemented.
fn handle_clipboard_stream(
    layout: &Arc<Mutex<ReceiverLayout>>,
    events: &EventSink,
    payload: &[u8],
) -> bool {
    let Ok(packet) = rmp_serde::from_slice::<ClipboardPacket>(payload) else {
        return false;
    };
    if packet.protocol != CLIPBOARD_PROTOCOL {
        return false;
    }

    let authorized = match layout.lock() {
        Ok(layout) => crate::state::input_authorized(
            &layout,
            &packet.cluster_id,
            &packet.pair_secret,
            &packet.origin_transport_public_key,
            &packet.origin_id,
        ),
        Err(_) => false,
    };
    if !authorized {
        log::warn!("clipboard packet rejected from {}", packet.origin_id);
        return true;
    }

    if !packet.text.is_empty() {
        log::info!("clipboard text received ({} chars)", packet.text.chars().count());
        events(ReceiverEvent::ClipboardText(packet.text));
        return true;
    }

    if packet.image.is_some() {
        log::info!("clipboard image ignored (not supported on Android yet)");
    }
    true
}

fn handle_pairing_stream(
    layout: &Arc<Mutex<ReceiverLayout>>,
    challenge: &SharedChallenge,
    events: &EventSink,
    events_tx: &Sender<ReceiverEvent>,
    payload: &[u8],
) -> bool {
    let Ok(packet) = rmp_serde::from_slice::<crate::packet::DiscoveryPacket>(payload) else {
        return false;
    };
    if packet.protocol != crate::packet::DISCOVERY_PROTOCOL || packet.kind != "pair-confirm" {
        return false;
    }

    let mut peer = packet.peer;
    if peer.quic_port == 0 {
        peer.quic_port = peer.transport_port;
    }
    if peer.protocol_version == 0 {
        peer.protocol_version = crate::packet::PROTOCOL_VERSION;
    }
    peer.last_seen_ms = now_ms();

    match complete_pairing_from_confirm(
        layout,
        challenge,
        &peer,
        packet.pairing_code,
        packet.pair_cluster_id,
        packet.pair_secret,
    ) {
        Ok(updated) => {
            log::info!("paired with controller {}", peer.name);
            let _ = events_tx.send(ReceiverEvent::Paired {
                controller_id: peer.id.clone(),
                controller_name: peer.name.clone(),
                layout: updated,
            });
            true
        }
        Err(error) => {
            log::warn!("pairing confirm rejected: {error}");
            events(ReceiverEvent::PairingFailed { reason: error });
            // Still consume the stream so the desktop gets an ack and can show
            // a meaningful error instead of a send timeout.
            true
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{InputEvent, PairedController};

    fn paired_layout() -> ReceiverLayout {
        let mut layout = ReceiverLayout::new_unpaired();
        layout.cluster_id = "cluster-1".into();
        layout.pair_secret = "secret-1".into();
        layout.paired_controllers.push(PairedController {
            id: "peer-desktop".into(),
            name: "Desktop".into(),
            host: "desktop".into(),
            ip: "192.168.42.1".into(),
            transport_public_key: "key-1".into(),
            protocol_version: 1,
            cluster_id: "cluster-1".into(),
            paired_at_ms: 0,
        });
        layout
    }

    fn input_payload(secret: &str) -> Vec<u8> {
        let packet = InputPacket {
            protocol: INPUT_PROTOCOL.into(),
            target_device_id: "peer-android-abcd".into(),
            origin_device_id: "peer-desktop".into(),
            origin_port: 47834,
            origin_transport_public_key: "key-1".into(),
            origin_protocol_version: 1,
            cluster_id: "cluster-1".into(),
            pair_secret: secret.into(),
            event: InputEvent::Key {
                key_code: 0x41,
                down: true,
            },
        };
        rmp_serde::to_vec_named(&packet).unwrap()
    }

    fn collect(layout: ReceiverLayout) -> (Arc<Mutex<OriginCache>>, Arc<Mutex<Vec<InputEvent>>>) {
        let _ = layout;
        (
            Arc::new(Mutex::new(OriginCache::default())),
            Arc::new(Mutex::new(Vec::new())),
        )
    }

    #[test]
    fn credentialled_packet_from_paired_controller_is_delivered() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let (cache, seen) = collect(paired_layout());
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                if let ReceiverEvent::Input(event) = event {
                    seen.lock().unwrap().push(event);
                }
            })
        };

        let source: SocketAddr = "192.168.42.1:47834".parse().unwrap();
        handle_input_datagram(&layout, &cache, &sink, &input_payload("secret-1"), source);

        assert_eq!(seen.lock().unwrap().len(), 1);
        // ...and the source is now authorized for credential-less datagrams.
        assert!(cache.lock().unwrap().is_fresh(source));
    }

    #[test]
    fn packet_with_a_wrong_secret_is_dropped() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let (cache, seen) = collect(paired_layout());
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                if let ReceiverEvent::Input(event) = event {
                    seen.lock().unwrap().push(event);
                }
            })
        };

        let source: SocketAddr = "192.168.42.1:47834".parse().unwrap();
        handle_input_datagram(&layout, &cache, &sink, &input_payload("wrong"), source);

        assert!(seen.lock().unwrap().is_empty());
        assert!(!cache.lock().unwrap().is_fresh(source));
    }

    /// The steady-state fast path: after one credentialled packet, the lean
    /// per-move datagrams (no secret attached) must still be accepted.
    #[test]
    fn credential_less_packet_is_accepted_after_authorization() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let (cache, seen) = collect(paired_layout());
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                if let ReceiverEvent::Input(event) = event {
                    seen.lock().unwrap().push(event);
                }
            })
        };
        let source: SocketAddr = "192.168.42.1:47834".parse().unwrap();

        handle_input_datagram(&layout, &cache, &sink, &input_payload("secret-1"), source);
        handle_input_datagram(&layout, &cache, &sink, &input_payload(""), source);

        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// A stranger that never proved the secret must not be able to inject just
    /// by omitting it.
    #[test]
    fn credential_less_packet_from_unknown_source_is_dropped() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let (cache, seen) = collect(paired_layout());
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                if let ReceiverEvent::Input(event) = event {
                    seen.lock().unwrap().push(event);
                }
            })
        };

        let source: SocketAddr = "192.168.42.99:47834".parse().unwrap();
        handle_input_datagram(&layout, &cache, &sink, &input_payload(""), source);

        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn unpaired_receiver_drops_credentialled_packets() {
        let layout = Arc::new(Mutex::new(ReceiverLayout::new_unpaired()));
        let (cache, seen) = collect(ReceiverLayout::new_unpaired());
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                if let ReceiverEvent::Input(event) = event {
                    seen.lock().unwrap().push(event);
                }
            })
        };

        let source: SocketAddr = "192.168.42.1:47834".parse().unwrap();
        handle_input_datagram(&layout, &cache, &sink, &input_payload("secret-1"), source);

        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn garbage_datagrams_are_ignored() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let (cache, seen) = collect(paired_layout());
        let sink: EventSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                if let ReceiverEvent::Input(event) = event {
                    seen.lock().unwrap().push(event);
                }
            })
        };

        let source: SocketAddr = "192.168.42.1:47834".parse().unwrap();
        handle_input_datagram(&layout, &cache, &sink, b"not messagepack", source);
        assert!(seen.lock().unwrap().is_empty());
    }
}
