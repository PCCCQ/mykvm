//! UDP discovery + pairing responder for the Android receiver.
//!
//! Ported from the desktop `start_discovery` loop, trimmed to what a
//! receiver-only peer needs: announce, answer probes, run the pairing handshake.
//!
//! Two Android-specific differences from the desktop:
//!   * Broadcast is unreliable (Android filters it aggressively and the Wi-Fi
//!     power save state drops packets), so the unicast /24 sweep is not an
//!     optional fallback here -- it runs on a slow timer forever.
//!   * The subnet broadcast address is derived from the interface netmask. The
//!     USB-tethering link is a /24 today, but a hardcoded `.255` would silently
//!     break on any other prefix.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::packet::{
    DiscoveryPairingFields, DiscoveryPacket, IncomingDiscovery, LanPeer, LanPeerScreen, Screen,
    DISCOVERY_PROTOCOL,
};
use crate::events::{EventSink, ReceiverEvent};
use crate::pairing::{begin_pairing_challenge, can_refresh_controller_identity, SharedChallenge};
use crate::state::{now_ms, ReceiverLayout};

/// Ports scanned upward from the base, mirroring `DISCOVERY_PORT_SPAN`.
const DISCOVERY_PORT_SPAN: u16 = 8;
const TRANSPORT_PORT_MAX: u16 = 65_535;
const TRANSPORT_PORT_MIN: u16 = 1024;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);
/// How often the interface list (and therefore the broadcast targets) is
/// re-read. Cheap enough to stay current, expensive enough to keep off the
/// per-packet path.
const INTERFACE_REFRESH: Duration = Duration::from_secs(30);
const RECV_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_DATAGRAM_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    /// Human readable name shown on the desktop ("Pixel 8").
    pub device_name: String,
    /// Stable identity for this phone. Must NOT contain the IP: the desktop
    /// keys its saved devices off this id, and a phone changes IP whenever it
    /// moves between Wi-Fi and USB tethering.
    pub stable_id: String,
    pub app_version: String,
    pub screen_id: String,
    /// Shared so the receiver can re-advertise a new size without tearing
    /// down the QUIC endpoint (which would drop every connection and make
    /// the desktop see CONNECTION_REFUSED).
    pub screen: Arc<Mutex<Screen>>,
    pub discovery_port: u16,
    pub quic_port: u16,
    pub transport_public_key: String,
    pub protocol_version: u16,
}

impl ReceiverConfig {
    /// Current screen geometry, copied out under the lock.
    pub fn screen(&self) -> Screen {
        match self.screen.lock() {
            Ok(screen) => screen.clone(),
            // A poisoned lock still holds valid data; the receiver must not
            // stop advertising a screen because something panicked once.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Re-advertises a new size.
    ///
    /// Callers update this instead of restarting the receiver: a restart tears
    /// down the QUIC endpoint, which drops every live connection and makes the
    /// desktop log "the server refused to accept a new connection" until the
    /// new endpoint is up.
    pub fn set_screen_size(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let mut screen = match self.screen.lock() {
            Ok(screen) => screen,
            Err(poisoned) => poisoned.into_inner(),
        };
        if screen.width == width && screen.height == height {
            return;
        }
        screen.width = width;
        screen.height = height;
        log::info!("advertised screen resized to {width}x{height}");
    }

    pub fn to_peer_screen(&self) -> LanPeerScreen {
        screen_to_peer_screen(&self.screen())
    }
}

// ---------------------------------------------------------------------------
// Where the paired desktop can be reached
// ---------------------------------------------------------------------------

/// The address we would dial to push something *to* the desktop (today: the
/// diagnostics upload). Discovery already learns every field of this; before,
/// they were thrown away as soon as the packet was handled.
#[derive(Debug, Clone)]
pub struct ControllerEndpoint {
    pub ip: String,
    pub quic_port: u16,
    pub transport_public_key: String,
    pub protocol_version: u16,
}

pub type SharedControllerEndpoint = Arc<Mutex<Option<ControllerEndpoint>>>;

// ---------------------------------------------------------------------------
// Events surfaced to the Kotlin layer (see events.rs)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Peer identity helpers (ported verbatim from the desktop)
// ---------------------------------------------------------------------------

pub fn local_peer_id(stable_id: &str) -> String {
    let normalized = stable_id
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    if normalized.is_empty() {
        "peer-android".into()
    } else {
        format!("peer-{normalized}")
    }
}

pub fn screen_to_peer_screen(screen: &Screen) -> LanPeerScreen {
    LanPeerScreen {
        id: screen.id.clone(),
        name: screen.name.clone(),
        x: screen.x,
        y: screen.y,
        width: screen.width,
        height: screen.height,
        scale: screen.scale,
        is_primary: screen.is_primary,
    }
}

pub fn encode_discovery_payload(
    kind: &str,
    local_peer: &LanPeer,
    pairing: DiscoveryPairingFields,
) -> Result<Vec<u8>, String> {
    let mut peer = local_peer.clone();
    peer.last_seen_ms = now_ms();
    let packet = DiscoveryPacket {
        protocol: DISCOVERY_PROTOCOL.into(),
        kind: kind.into(),
        peer,
        pairing_code: pairing.code,
        pair_cluster_id: pairing.cluster_id,
        pair_secret: pairing.secret,
        pairing_error: pairing.error,
    };
    rmp_serde::to_vec_named(&packet).map_err(|error| error.to_string())
}

pub fn decode_discovery_packet(payload: &[u8]) -> Option<DiscoveryPacket> {
    let packet = rmp_serde::from_slice::<DiscoveryPacket>(payload).ok()?;
    (packet.protocol == DISCOVERY_PROTOCOL).then_some(packet)
}

pub fn peer_from_discovery_packet(
    packet: DiscoveryPacket,
    source_ip: String,
    local_peer_id: &str,
) -> Option<IncomingDiscovery> {
    if packet.peer.id == local_peer_id {
        return None;
    }

    let mut peer = packet.peer;
    // The advertised address is never trusted -- the datagram's source is.
    peer.ip = source_ip;
    if peer.quic_port == 0 {
        peer.quic_port = peer.transport_port;
    }
    if peer.protocol_version == 0 {
        peer.protocol_version = crate::packet::default_protocol_version();
    }
    if peer.transport_public_key.trim().is_empty()
        || peer.protocol_version != crate::packet::PROTOCOL_VERSION
    {
        peer.input_ready = false;
    }
    peer.last_seen_ms = now_ms();
    Some(IncomingDiscovery {
        kind: packet.kind,
        peer,
        pairing_code: packet.pairing_code,
        pair_cluster_id: packet.pair_cluster_id,
        pair_secret: packet.pair_secret,
    })
}

/// Mirrors `peer_visible_to_layout` for a receiver (`machine_role == "client"`).
fn peer_visible_to_layout(layout: &ReceiverLayout, peer: &LanPeer) -> bool {
    if peer.pairing_required {
        return layout.machine_role == "server";
    }

    let cluster_id = layout.cluster_id.trim();
    !cluster_id.is_empty() && peer.cluster_id == cluster_id
}

pub fn is_paired_controller(layout: &ReceiverLayout, peer: &LanPeer) -> bool {
    layout
        .paired_controllers
        .iter()
        .any(|controller| crate::pairing::identity_matches_peer(controller, peer))
}

/// Mirrors `should_reply_to_discovery`.
fn should_reply_to_discovery(layout: &ReceiverLayout, peer: &LanPeer) -> bool {
    if peer_visible_to_layout(layout, peer) {
        return true;
    }
    if !layout.pairing_required() {
        return is_paired_controller(layout, peer);
    }
    peer.machine_role == "server"
}

// ---------------------------------------------------------------------------
// Network helpers
// ---------------------------------------------------------------------------

/// Every usable non-loopback IPv4 address, with the default-route address
/// first. The default route is what packets actually egress through, so it is
/// the address the desktop will see as our source.
pub fn local_ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut addresses = Vec::new();

    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if interface.is_loopback() {
                continue;
            }
            let if_addrs::IfAddr::V4(address) = interface.addr else {
                continue;
            };
            if usable_discovery_ipv4(address.ip) {
                addresses.push(address.ip);
            }
        }
    }

    if let Some(default_ip) = default_route_ipv4_address() {
        if usable_discovery_ipv4(default_ip) {
            addresses.insert(0, default_ip);
        }
    }

    addresses.sort_by_key(|address| address.octets());
    addresses.dedup();
    addresses
}

fn default_route_ipv4_address() -> Option<Ipv4Addr> {
    // Connecting a UDP socket sends nothing; it only makes the kernel pick the
    // route (and therefore the source address) for this destination.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) => Some(address),
        _ => None,
    }
}

pub fn usable_discovery_ipv4(address: Ipv4Addr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && !address.is_broadcast()
        && !address.is_link_local()
}

pub fn discovery_target_ports(base: u16) -> Vec<u16> {
    let base = base.max(TRANSPORT_PORT_MIN);
    let mut ports = Vec::new();
    for offset in 0..DISCOVERY_PORT_SPAN {
        let Some(port) = base.checked_add(offset) else {
            break;
        };
        if port > TRANSPORT_PORT_MAX {
            break;
        }
        ports.push(port);
    }
    ports
}

/// Global broadcast plus the real broadcast address of each interface.
///
/// Computed from the netmask rather than assumed to be `.255` so the USB
/// tethering link keeps working if Android ever ships a non-/24 prefix there.
pub fn broadcast_targets(base_port: u16, local_ips: &[Ipv4Addr]) -> Vec<String> {
    let mut addresses = Vec::new();
    let subnet_broadcasts = subnet_broadcast_addresses();

    for port in discovery_target_ports(base_port) {
        addresses.push(format!("255.255.255.255:{port}"));
        for broadcast in &subnet_broadcasts {
            addresses.push(format!("{broadcast}:{port}"));
        }
        // Fall back to the /24 assumption when netmasks were unavailable.
        if subnet_broadcasts.is_empty() {
            for ip in local_ips {
                let [a, b, c, _] = ip.octets();
                addresses.push(format!("{a}.{b}.{c}.255:{port}"));
            }
        }
    }

    addresses.sort();
    addresses.dedup();
    addresses
}

fn subnet_broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut broadcasts = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if interface.is_loopback() {
                continue;
            }
            let if_addrs::IfAddr::V4(address) = interface.addr else {
                continue;
            };
            if !usable_discovery_ipv4(address.ip) {
                continue;
            }
            let netmask = u32::from(address.netmask);
            if netmask == 0 {
                continue;
            }
            let broadcast = Ipv4Addr::from(u32::from(address.ip) | !netmask);
            if usable_discovery_ipv4(broadcast) {
                broadcasts.push(broadcast);
            }
        }
    }
    broadcasts.sort_by_key(|address| address.octets());
    broadcasts.dedup();
    broadcasts
}


pub fn bind_available_udp_port(preferred: u16) -> Result<(UdpSocket, u16), String> {
    let start = preferred.max(TRANSPORT_PORT_MIN);
    for offset in 0..64_u16 {
        let candidate = start.saturating_add(offset);
        if candidate > TRANSPORT_PORT_MAX {
            break;
        }
        if let Ok(socket) = bind_reusable_udp_port(candidate) {
            return Ok((socket, candidate));
        }
    }

    let socket = bind_reusable_udp_port(0)
        .map_err(|error| format!("failed to bind any discovery port: {error}"))?;
    let port = socket
        .local_addr()
        .map_err(|error| format!("failed to read discovery port: {error}"))?
        .port();
    Ok((socket, port))
}

fn bind_reusable_udp_port(port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    socket.bind(&address.into())?;
    let socket: UdpSocket = socket.into();
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(RECV_TIMEOUT))?;
    Ok(socket)
}

// ---------------------------------------------------------------------------
// Announce payload assembly
// ---------------------------------------------------------------------------

pub fn local_peer(config: &ReceiverConfig, layout: &ReceiverLayout, local_ip: &str) -> LanPeer {
    LanPeer {
        id: local_peer_id(&config.stable_id),
        name: config.device_name.clone(),
        platform: "android".into(),
        machine_role: "client".into(),
        cluster_id: layout.advertised_cluster_id(),
        pairing_required: layout.pairing_required(),
        // Stable host label: the desktop stores device.host and uses it to
        // re-match the phone after its IP changes.
        host: config.stable_id.clone(),
        ip: local_ip.to_string(),
        transport_port: config.discovery_port,
        quic_port: config.quic_port,
        transport_public_key: config.transport_public_key.clone(),
        protocol_version: config.protocol_version,
        screen_count: 1,
        input_ready: layout.input_ready(),
        upgrading: false,
        screens: vec![config.to_peer_screen()],
        app_version: config.app_version.clone(),
        last_seen_ms: now_ms(),
    }
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

pub struct DiscoveryHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl DiscoveryHandle {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spawn_discovery(
    config: ReceiverConfig,
    layout: Arc<Mutex<ReceiverLayout>>,
    challenge: SharedChallenge,
    endpoint: SharedControllerEndpoint,
    events: EventSink,
) -> Result<DiscoveryHandle, String> {
    let (socket, actual_port) = bind_available_udp_port(config.discovery_port)?;
    let stop = Arc::new(AtomicBool::new(false));
    log::info!("discovery listening on UDP {actual_port}");

    let stop_for_thread = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("mykvm-discovery".into())
        .spawn(move || {
            let mut config = config;
            config.discovery_port = actual_port;
            run_discovery(
                config,
                layout,
                challenge,
                endpoint,
                events,
                socket,
                stop_for_thread,
            );
        })
        .map_err(|error| format!("failed to spawn discovery thread: {error}"))?;

    Ok(DiscoveryHandle {
        stop,
        join: Some(join),
    })
}

fn run_discovery(
    config: ReceiverConfig,
    layout: Arc<Mutex<ReceiverLayout>>,
    challenge: SharedChallenge,
    endpoint: SharedControllerEndpoint,
    events: EventSink,
    socket: UdpSocket,
    stop: Arc<AtomicBool>,
) {
    let mut buffer = [0_u8; MAX_DATAGRAM_BYTES];
    let mut last_announce = Instant::now() - ANNOUNCE_INTERVAL;
    let mut last_interface_refresh = Instant::now() - INTERFACE_REFRESH;
    let mut local_ips: Vec<Ipv4Addr> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    let mut online_controllers: Vec<String> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        // Enumerating interfaces is a getifaddrs() walk and a subnet-broadcast
        // calculation per interface. The LAN layout changes on the scale of
        // minutes, so keep it off the per-packet path.
        if last_interface_refresh.elapsed() >= INTERFACE_REFRESH {
            local_ips = local_ipv4_addresses();
            targets = broadcast_targets(config.discovery_port, &local_ips);
            last_interface_refresh = Instant::now();
            log::info!(
                "discovery interfaces: {local_ips:?} ({} broadcast targets)",
                targets.len()
            );
        }

        let local_ip = local_ips
            .first()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "0.0.0.0".into());

        if last_announce.elapsed() >= ANNOUNCE_INTERVAL {
            announce(&socket, &config, &layout, &local_ip, &targets);
            expire_pairing_code(&challenge, &events);
            last_announce = Instant::now();
        }

        // Reading is this loop's real job: the desktop's probe is answered from
        // here, and the read timeout is what paces the announce timer above.
        let Ok((length, source)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Some(packet) = decode_discovery_packet(&buffer[..length]) else {
            continue;
        };

        let Ok(current) = layout.lock().map(|layout| layout.clone()) else {
            continue;
        };
        let peer_id = local_peer_id(&config.stable_id);
        let Some(incoming) =
            peer_from_discovery_packet(packet, source.ip().to_string(), &peer_id)
        else {
            // Either our own announce looped back or the datagram was
            // malformed; neither is worth a log line on every announce tick.
            continue;
        };
        let reply_peer = local_peer(&config, &current, &local_ip);

        // Keep the stored controller record -- and the address we would dial to
        // reach the desktop -- in sync with the peer actually on the wire. The
        // desktop regenerates its self-signed certificate when its key file is
        // missing (update/reinstall) and its IP moves between Wi-Fi and USB
        // tethering; without this sync the stored record goes stale, every input
        // packet is rejected, and the user is forced to re-pair.
        adopt_controller(&layout, &incoming.peer, &endpoint, &events, &mut online_controllers);

        log::debug!(
            "discovery {} from {} peer={} role={} pairing_required={}",
            incoming.kind,
            source,
            incoming.peer.id,
            incoming.peer.machine_role,
            incoming.peer.pairing_required
        );

        match incoming.kind.as_str() {
            "pair-request" => {
                // A desktop wants to pair; mint a code and show it on screen.
                let issued = begin_pairing_challenge(
                    &challenge,
                    &current,
                    &incoming.peer,
                    source.ip().to_string(),
                );
                match issued {
                    Some(issued) => {
                        let reply = encode_discovery_payload(
                            "pair-challenge",
                            &reply_peer,
                            DiscoveryPairingFields::default(),
                        );
                        match reply {
                            Ok(payload) => {
                                let sent = socket.send_to(&payload, source);
                                log::info!(
                                    "pairing requested by {} ({}); challenge {} (sent={:?})",
                                    issued.requester_name,
                                    source,
                                    issued.code,
                                    sent.is_ok()
                                );
                            }
                            Err(error) => log::warn!("failed to encode pair-challenge: {error}"),
                        }
                        events(ReceiverEvent::PairingRequested {
                            code: issued.code.clone(),
                            requester_name: issued.requester_name.clone(),
                            requester_ip: issued.requester_ip.clone(),
                            expires_at_ms: issued.expires_at_ms,
                        });
                    }
                    None => log::warn!(
                        "ignored a pair-request from {} (role={}); only an unpaired \
                         receiver accepts a server's request",
                        source,
                        incoming.peer.machine_role
                    ),
                }
            }
            "pair-confirm" => {
                // Arrives over the encrypted QUIC stream, not here. Ignore any
                // plaintext UDP copy so a spoofed datagram cannot pair us.
                log::warn!("ignored a plaintext pair-confirm from {source}");
            }
            "announce" | "probe" => {
                if should_reply_to_discovery(&current, &incoming.peer) {
                    match encode_discovery_payload(
                        "reply",
                        &reply_peer,
                        DiscoveryPairingFields::default(),
                    ) {
                        Ok(payload) => {
                            if let Err(error) = socket.send_to(&payload, source) {
                                log::warn!("failed to reply to {source}: {error}");
                            }
                        }
                        Err(error) => log::warn!("failed to encode reply: {error}"),
                    }
                }
            }
            "reply" | "pair-challenge" => {}
            other => log::debug!("ignoring discovery packet kind={other} from {source}"),
        }
    }

    log::info!("discovery loop stopped");
}

/// Sends one announce to every broadcast target.
///
/// Deliberately *not* accompanied by a unicast sweep: a receiver is a passive
/// responder, and the desktop already sweeps every subnet it can see. Sweeping
/// from here meant 2000+ datagrams to non-existent hosts every 15s -- each one
/// an ARP round trip -- which stalled this loop badly enough to drop probes and
/// burn battery for nothing.
fn announce(
    socket: &UdpSocket,
    config: &ReceiverConfig,
    layout: &Arc<Mutex<ReceiverLayout>>,
    local_ip: &str,
    targets: &[String],
) {
    let peer = layout
        .lock()
        .map(|layout| local_peer(config, &layout, local_ip))
        .unwrap_or_else(|_| local_peer(config, &ReceiverLayout::new_unpaired(), local_ip));

    let Ok(payload) = encode_discovery_payload("announce", &peer, DiscoveryPairingFields::default())
    else {
        return;
    };

    for target in targets {
        if let Err(error) = socket.send_to(&payload, target) {
            log::debug!("announce to {target} failed: {error}");
        }
    }
}

/// Clears the on-screen pairing code once it can no longer be used.
fn expire_pairing_code(challenge: &SharedChallenge, events: &EventSink) {
    let expired = challenge
        .lock()
        .ok()
        .map(|mut guard| {
            let expired = guard.as_ref().is_some_and(|pending| pending.is_expired());
            if expired {
                *guard = None;
            }
            expired
        })
        .unwrap_or(false);

    if expired {
        events(ReceiverEvent::PairingCleared);
    }
}

/// Mirrors the desktop's `refresh_paired_controller_keys`.
///
/// Two jobs, both about the same peer:
///   * refresh the stored `PairedController` (id / key / host / ip / name) so a
///     rotated certificate or a moved IP does not lock input out, and
///   * remember the address to dial for the diagnostics upload.
/// Plus the presence bookkeeping the UI uses to show "desktop connected".
///
/// The refresh is gated on [`can_refresh_controller_identity`], deliberately
/// stricter than `can_repair_with_peer`: rewriting stored credentials on an
/// IP-only match would let a second MyKVM instance on the same PC take over the
/// pairing record.
fn adopt_controller(
    layout: &Arc<Mutex<ReceiverLayout>>,
    peer: &LanPeer,
    endpoint: &SharedControllerEndpoint,
    events: &EventSink,
    online: &mut Vec<String>,
) {
    // Only the desktop (a server) is ever a controller for us.
    if peer.machine_role != "server" {
        return;
    }
    if peer.ip.trim().is_empty() || peer.quic_port == 0 {
        return;
    }

    let Ok(mut layout) = layout.lock() else {
        return;
    };
    if layout.machine_role != "client" {
        return;
    }

    let index = layout
        .paired_controllers
        .iter()
        .position(|controller| can_refresh_controller_identity(controller, peer));

    // Remember where to dial even when the stored record no longer matches: a
    // desktop whose certificate rotated *and* whose hostname changed is exactly
    // the case where the user needs to re-pair, and re-pairing is also when they
    // want to send us its logs. Presenting our cluster id is hint enough, and the
    // desktop still validates the credentials on receipt.
    let cluster_matches =
        !layout.cluster_id.trim().is_empty() && peer.cluster_id == layout.cluster_id;
    if (index.is_some() || cluster_matches) && !peer.transport_public_key.trim().is_empty() {
        if let Ok(mut slot) = endpoint.lock() {
            *slot = Some(ControllerEndpoint {
                ip: peer.ip.clone(),
                quic_port: peer.quic_port,
                transport_public_key: peer.transport_public_key.clone(),
                protocol_version: peer.protocol_version,
            });
        }
    }

    let Some(index) = index else {
        return;
    };
    let controller = &mut layout.paired_controllers[index];

    let mut changed = false;
    let set = |slot: &mut String, value: &str| {
        let value = value.trim();
        if !value.is_empty() && slot.as_str() != value {
            *slot = value.to_string();
            true
        } else {
            false
        }
    };
    changed |= set(&mut controller.name, &peer.name);
    changed |= set(&mut controller.host, &peer.host);
    changed |= set(&mut controller.ip, &peer.ip);
    changed |= set(&mut controller.id, &peer.id);
    changed |= set(
        &mut controller.transport_public_key,
        &peer.transport_public_key,
    );
    if peer.protocol_version != 0 && controller.protocol_version != peer.protocol_version {
        controller.protocol_version = peer.protocol_version;
        changed = true;
    }

    let controller_id = controller.id.clone();
    let controller_name = if controller.name.trim().is_empty() {
        controller_id.clone()
    } else {
        controller.name.clone()
    };
    let snapshot = layout.clone();
    drop(layout);

    if !online.contains(&controller_id) {
        online.push(controller_id.clone());
        events(ReceiverEvent::PeerPresence {
            peer_ids: online.clone(),
        });
    }
    if changed {
        log::info!(
            "paired controller {controller_id} identity refreshed from {} (key/host/id changed)",
            peer.ip
        );
        // Reuse the pairing event: Kotlin persists the layout verbatim, which is
        // exactly what a refreshed record needs.
        events(ReceiverEvent::Paired {
            controller_id,
            controller_name,
            layout: snapshot,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ReceiverConfig {
        ReceiverConfig {
            device_name: "Pixel 8".into(),
            stable_id: "android-abcd1234".into(),
            app_version: "0.1.0".into(),
            screen_id: "android-screen-1".into(),
            screen: Arc::new(Mutex::new(Screen {
                id: "android-screen-1".into(),
                device_id: "android-abcd1234".into(),
                name: "Pixel 8".into(),
                x: 0,
                y: 0,
                width: 1080,
                height: 2400,
                scale: 1.0,
                is_primary: true,
            })),
            discovery_port: 47833,
            quic_port: 47834,
            transport_public_key: "key".into(),
            protocol_version: crate::packet::PROTOCOL_VERSION,
        }
    }

    #[test]
    fn resizing_the_shared_screen_is_visible_to_the_next_announce() {
        let config = test_config();
        assert_eq!(config.screen().width, 1080);

        config.set_screen_size(2944, 1840);
        assert_eq!(config.screen().width, 2944);
        assert_eq!(config.screen().height, 1840);
        assert_eq!(config.to_peer_screen().width, 2944);

        // Garbage sizes must not be advertised.
        config.set_screen_size(0, -3);
        assert_eq!(config.screen().width, 2944);
    }

    fn paired_layout() -> ReceiverLayout {
        let mut layout = ReceiverLayout::new_unpaired();
        layout.cluster_id = "cluster-1".into();
        layout.pair_secret = "secret-1".into();
        layout.paired_controllers.push(crate::packet::PairedController {
            id: "peer-desktop-192-168-1-7".into(),
            name: "Desktop".into(),
            host: "desktop".into(),
            ip: "192.168.1.7".into(),
            transport_public_key: "old-key".into(),
            protocol_version: 1,
            cluster_id: "cluster-1".into(),
            paired_at_ms: 0,
        });
        layout
    }

    fn desktop_peer(id: &str, ip: &str, key: &str) -> LanPeer {
        LanPeer {
            id: id.into(),
            name: "Desktop".into(),
            platform: "windows".into(),
            machine_role: "server".into(),
            cluster_id: "cluster-1".into(),
            pairing_required: false,
            host: "desktop".into(),
            ip: ip.into(),
            transport_port: 47833,
            quic_port: 47834,
            transport_public_key: key.into(),
            protocol_version: crate::packet::PROTOCOL_VERSION,
            screen_count: 0,
            input_ready: true,
            upgrading: false,
            screens: vec![],
            app_version: "0.1.0".into(),
            last_seen_ms: 0,
        }
    }

    fn no_events() -> EventSink {
        Arc::new(|_| {})
    }

    /// The Android twin of the desktop's `refresh_paired_controller_keys`: a
    /// rotated certificate or a moved IP must not leave input locked out.
    #[test]
    fn a_rotated_controller_identity_is_adopted_in_place() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let endpoint: SharedControllerEndpoint = Arc::new(Mutex::new(None));
        let mut online = Vec::new();

        adopt_controller(
            &layout,
            &desktop_peer("peer-desktop-192-168-1-9", "192.168.1.9", "new-key"),
            &endpoint,
            &no_events(),
            &mut online,
        );

        let stored = layout.lock().unwrap();
        assert_eq!(stored.paired_controllers.len(), 1, "must not add a second controller");
        let controller = &stored.paired_controllers[0];
        assert_eq!(controller.transport_public_key, "new-key");
        assert_eq!(controller.ip, "192.168.1.9");
        assert_eq!(controller.id, "peer-desktop-192-168-1-9");
        drop(stored);

        // ...and the address the diagnostics upload dials is now current.
        let endpoint = endpoint.lock().unwrap().clone().expect("endpoint published");
        assert_eq!(endpoint.ip, "192.168.1.9");
        assert_eq!(endpoint.transport_public_key, "new-key");
        assert_eq!(online, vec!["peer-desktop-192-168-1-9".to_string()]);
    }

    /// Regression, found on the real tablet: the desktop app and a second MyKVM
    /// instance (the live test controller) ran on the same PC, so they shared an
    /// IP. Matching on the IP alone let the test controller overwrite the stored
    /// controller -- credentials and all. Only an identity or hostname match may
    /// rewrite the record.
    #[test]
    fn a_second_instance_on_the_same_pc_cannot_take_over_the_record() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let endpoint: SharedControllerEndpoint = Arc::new(Mutex::new(None));
        let mut online = Vec::new();

        // Same PC, same IP, different name/host/id/key.
        let mut impostor = desktop_peer("peer-live-controller", "192.168.1.7", "live-key");
        impostor.name = "Live Test Controller".into();
        impostor.host = "live-controller-host".into();

        adopt_controller(&layout, &impostor, &endpoint, &no_events(), &mut online);

        let stored = layout.lock().unwrap();
        let controller = &stored.paired_controllers[0];
        assert_eq!(controller.transport_public_key, "old-key");
        assert_eq!(controller.id, "peer-desktop-192-168-1-7");
        assert_eq!(controller.name, "Desktop");
        assert!(online.is_empty(), "the impostor must not look like our controller");
    }

    /// A stranger must not be able to overwrite the record or become the upload
    /// target just by being on the same LAN.
    #[test]
    fn an_unrelated_desktop_is_left_alone() {
        let layout = Arc::new(Mutex::new(paired_layout()));
        let endpoint: SharedControllerEndpoint = Arc::new(Mutex::new(None));
        let mut online = Vec::new();

        let mut stranger = desktop_peer("peer-other-10-0-0-5", "10.0.0.5", "other-key");
        stranger.name = "Other PC".into();
        stranger.host = "other-pc".into();
        // Not our cluster, no matching identity: nothing to adopt.
        stranger.cluster_id = "cluster-other".into();

        adopt_controller(&layout, &stranger, &endpoint, &no_events(), &mut online);

        let stored = layout.lock().unwrap();
        assert_eq!(stored.paired_controllers[0].transport_public_key, "old-key");
        assert_eq!(stored.paired_controllers[0].ip, "192.168.1.7");
        drop(stored);
        assert!(endpoint.lock().unwrap().is_none(), "no endpoint for a stranger");
        assert!(online.is_empty());
    }

    #[test]
    fn peer_id_is_stable_and_ip_independent() {
        // The desktop keys saved devices off this id, so it must not move when
        // the phone switches between Wi-Fi and USB tethering.
        assert_eq!(local_peer_id("android-abcd1234"), "peer-android-abcd1234");
        assert_eq!(
            local_peer_id("Android-ABCD1234"),
            local_peer_id("android-abcd1234")
        );
    }

    #[test]
    fn discovery_port_span_covers_neighbouring_ports() {
        let ports = discovery_target_ports(47833);
        assert_eq!(ports.len(), DISCOVERY_PORT_SPAN as usize);
        assert_eq!(ports[0], 47833);
        assert!(ports.contains(&47834));
    }

    #[test]
    fn broadcast_targets_include_global_and_subnet_addresses() {
        let targets = broadcast_targets(47833, &[Ipv4Addr::new(192, 168, 42, 129)]);
        assert!(targets.contains(&"255.255.255.255:47833".to_string()));
        // The netmask-derived subnet broadcast may be empty in a sandbox, in
        // which case the /24 fallback must still be present.
        assert!(targets.iter().any(|target| target.ends_with(".255:47833")));
    }

    #[test]
    fn unpaired_receiver_answers_only_servers() {
        let layout = ReceiverLayout::new_unpaired();
        let mut server = local_peer(&test_config(), &layout, "192.168.42.129");
        server.id = "peer-desktop".into();
        server.machine_role = "server".into();
        server.pairing_required = false;
        server.cluster_id = "cluster-1".into();

        let mut other_client = server.clone();
        other_client.machine_role = "client".into();

        assert!(should_reply_to_discovery(&layout, &server));
        assert!(!should_reply_to_discovery(&layout, &other_client));
    }

    /// The discovery reply is not the security boundary: any peer that knows
    /// the shared cluster id is answered (that is how the desktop behaves, and
    /// how two devices in the same cluster find each other). What actually
    /// gates input is `paired_controllers`, checked in `input_authorized`.
    #[test]
    fn paired_receiver_answers_its_cluster_but_only_injects_for_its_controller() {
        let mut layout = ReceiverLayout::new_unpaired();
        layout.cluster_id = "cluster-1".into();
        layout.pair_secret = "secret-1".into();
        layout.paired_controllers.push(crate::packet::PairedController {
            id: "peer-desktop".into(),
            name: "Desktop".into(),
            host: "desktop".into(),
            ip: "192.168.42.1".into(),
            transport_public_key: "key-1".into(),
            protocol_version: 1,
            cluster_id: "cluster-1".into(),
            paired_at_ms: 0,
        });

        let mut controller = local_peer(&test_config(), &layout, "192.168.42.129");
        controller.id = "peer-desktop".into();
        controller.machine_role = "server".into();
        controller.pairing_required = false;
        controller.cluster_id = "cluster-1".into();

        // Same cluster, but not the controller we paired with.
        let mut cluster_mate = controller.clone();
        cluster_mate.id = "peer-other".into();
        cluster_mate.transport_public_key = "key-2".into();

        // A device from a different cluster is invisible to us entirely.
        let mut stranger = controller.clone();
        stranger.id = "peer-stranger".into();
        stranger.cluster_id = "cluster-2".into();

        assert!(should_reply_to_discovery(&layout, &controller));
        assert!(should_reply_to_discovery(&layout, &cluster_mate));
        assert!(!should_reply_to_discovery(&layout, &stranger));

        // ...but the cluster mate still cannot inject.
        assert!(crate::state::input_authorized(&layout, "cluster-1", "secret-1", "key-1", ""));
        assert!(!crate::state::input_authorized(&layout, "cluster-1", "secret-1", "key-2", "peer-other"));
    }

    #[test]
    fn announce_payload_round_trips_through_the_wire_format() {
        let layout = ReceiverLayout::new_unpaired();
        let peer = local_peer(&test_config(), &layout, "192.168.42.129");
        let payload =
            encode_discovery_payload("announce", &peer, DiscoveryPairingFields::default()).unwrap();
        let decoded = decode_discovery_packet(&payload).expect("decodes");

        assert_eq!(decoded.kind, "announce");
        assert_eq!(decoded.peer.platform, "android");
        assert_eq!(decoded.peer.machine_role, "client");
        assert!(decoded.peer.pairing_required);
        assert!(!decoded.peer.input_ready);
        assert_eq!(decoded.peer.screens.len(), 1);
        assert_eq!(decoded.peer.screens[0].width, 1080);
        assert_eq!(decoded.peer.screens[0].scale, 1.0);
    }

    #[test]
    fn a_packet_from_ourselves_is_ignored() {
        let layout = ReceiverLayout::new_unpaired();
        let peer = local_peer(&test_config(), &layout, "192.168.42.129");
        let payload =
            encode_discovery_payload("announce", &peer, DiscoveryPairingFields::default()).unwrap();
        let decoded = decode_discovery_packet(&payload).unwrap();
        let mine = local_peer_id("android-abcd1234");

        assert!(peer_from_discovery_packet(decoded, "192.168.42.129".into(), &mine).is_none());
    }

    /// Live smoke test against a receiver that is actually running on the LAN.
    ///
    /// Skipped unless `MYKVM_LIVE_PEER` is set, so CI stays hermetic:
    ///
    /// ```text
    /// MYKVM_LIVE_PEER=192.168.1.11:47833 cargo test -- --nocapture live_probe
    /// ```
    ///
    /// This plays the desktop's side of the handshake -- it sends the same
    /// `probe` datagram the desktop sends and asserts that the receiver answers
    /// with a peer that is a *client*, on *android*, that has a screen, and that
    /// is asking to be paired. It is the closest thing to an end-to-end test
    /// that does not need the desktop app running.
    ///
    /// Only meaningful against an **unpaired** receiver. A paired one answers
    /// nothing but its paired controller, and this test announces a throwaway
    /// identity -- so a silent timeout here against a paired device is correct
    /// behaviour, not a regression. Use `examples/live_controller` instead: it
    /// persists a real controller identity, so it can re-pair and then drive input.
    #[test]
    fn live_probe_finds_a_running_receiver() {
        let Ok(target) = std::env::var("MYKVM_LIVE_PEER") else {
            eprintln!("MYKVM_LIVE_PEER not set; skipping the live receiver probe");
            return;
        };

        let socket = UdpSocket::bind("0.0.0.0:0").expect("bind probe socket");
        socket
            .set_read_timeout(Some(Duration::from_millis(1500)))
            .expect("set read timeout");

        // Stand in for the desktop: a "server" (controller) with its own
        // cluster, probing for peers.
        let controller = LanPeer {
            id: "peer-live-test-controller".into(),
            name: "Live Test Controller".into(),
            platform: "windows".into(),
            machine_role: "server".into(),
            cluster_id: "cluster-live-test".into(),
            pairing_required: false,
            host: "live-test".into(),
            ip: "0.0.0.0".into(),
            transport_port: 47833,
            quic_port: 47834,
            transport_public_key: "live-test-key".into(),
            protocol_version: crate::packet::PROTOCOL_VERSION,
            screen_count: 1,
            input_ready: true,
            upgrading: false,
            screens: vec![],
            app_version: "live-test".into(),
            last_seen_ms: 0,
        };

        let payload =
            encode_discovery_payload("probe", &controller, DiscoveryPairingFields::default())
                .expect("encode probe");

        // Retry: the receiver's discovery loop only reads between announces, and
        // a first datagram can be lost while the Wi-Fi radio wakes up.
        let mut received = None;
        for _ in 0..5 {
            socket.send_to(&payload, &target).expect("send probe");
            let mut buffer = [0_u8; MAX_DATAGRAM_BYTES];
            if let Ok((length, source)) = socket.recv_from(&mut buffer) {
                if let Some(packet) = decode_discovery_packet(&buffer[..length]) {
                    received = Some((packet, source));
                    break;
                }
            }
        }

        let (packet, source) = received.expect(
            "no reply from the receiver -- is the service running and is UDP 47833 reachable?",
        );

        println!(
            "reply from {source}: kind={} id={} name={} platform={} role={} pairing_required={} screens={} quic_port={}",
            packet.kind,
            packet.peer.id,
            packet.peer.name,
            packet.peer.platform,
            packet.peer.machine_role,
            packet.peer.pairing_required,
            packet.peer.screens.len(),
            packet.peer.quic_port,
        );

        assert_eq!(packet.protocol, DISCOVERY_PROTOCOL);
        assert_eq!(packet.peer.machine_role, "client", "receiver must be a client");
        assert_eq!(packet.peer.platform, "android");
        assert!(!packet.peer.name.trim().is_empty());
        assert_eq!(
            packet.peer.screens.len(),
            1,
            "the receiver must advertise exactly one screen"
        );
        assert!(
            packet.peer.screens[0].width > 0 && packet.peer.screens[0].height > 0,
            "advertised screen must have a real size"
        );
        assert_eq!(packet.peer.screens[0].scale, 1.0);
        // A fresh receiver has no controller yet, so the desktop must be told
        // to run the pairing handshake before it can send input.
        assert!(packet.peer.pairing_required);
        assert!(!packet.peer.input_ready);
        // The port the desktop will dial for input.
        assert!(packet.peer.quic_port > 0);
        assert!(!packet.peer.transport_public_key.is_empty());
    }
}
