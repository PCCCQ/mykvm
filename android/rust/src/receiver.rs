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

use crate::discovery::{spawn_discovery, DiscoveryHandle, ReceiverConfig};
use crate::events::{EventSink, ReceiverEvent};
use crate::packet::{InputPacket, INPUT_PROTOCOL};
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
                handle_pairing_stream(&layout, &challenge, &events, &events_tx, &payload)
            })
        };

        let transport =
            quic_transport::start(startup.quic_port, startup.identity_dir, on_datagram, on_stream)?;

        let config = ReceiverConfig {
            device_name: startup.device_name,
            stable_id: startup.stable_id,
            app_version: startup.app_version,
            screen_id: startup.screen_id,
            screen_width: startup.screen_width,
            screen_height: startup.screen_height,
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

        let discovery = spawn_discovery(
            config.clone(),
            Arc::clone(&layout),
            Arc::clone(&challenge),
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
        })
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
    pub fn stop(&mut self) {
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
