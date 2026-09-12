//! Pairing handshake, ported from the desktop `begin_pairing_challenge` /
//! `complete_pairing_from_confirm`.
//!
//! Flow (all initiated by the desktop):
//!   1. desktop -> receiver : `pair-request`      (UDP discovery)
//!   2. receiver -> desktop : `pair-challenge`    (UDP discovery, shows this code)
//!   3. user types the 6 digit code into the desktop
//!   4. desktop -> receiver : `pair-confirm`      (QUIC stream, carries the
//!                                                 cluster id + pair secret)
//!
//! Step 4 arrives on the *encrypted* stream, not on discovery, and it is also
//! what makes the two sides agree on `cluster_id`/`pair_secret` -- the desktop's
//! input packets are only accepted when both match.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::packet::{LanPeer, PairedController};
use crate::state::{now_ms, random_pairing_code, ReceiverLayout};

/// Must match `PAIRING_CODE_TTL_MS` in the desktop app.
pub const PAIRING_CODE_TTL_MS: u64 = 60_000;
/// Must match `PAIRING_MAX_ATTEMPTS` in the desktop app.
pub const PAIRING_MAX_ATTEMPTS: u8 = 5;

#[derive(Debug, Clone)]
pub struct PairingChallenge {
    pub code: String,
    pub requester_id: String,
    pub requester_name: String,
    pub requester_ip: String,
    pub requester_host: String,
    pub requester_public_key: String,
    pub requester_protocol_version: u16,
    pub expires_at: Instant,
    pub expires_at_ms: u64,
    pub attempts: u8,
}

impl PairingChallenge {
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Instant::now()
    }
}

pub type SharedChallenge = Arc<Mutex<Option<PairingChallenge>>>;

/// Starts the handshake for a desktop that wants to pair (or re-pair) with us.
///
/// A *paired* receiver still accepts a fresh request. The authorization in this
/// protocol is the code: it is shown on this device's screen and typed on the
/// desktop, so an attacker on the LAN cannot finish a pairing without reading
/// the screen. Refusing while already paired used to make repair impossible --
/// once the stored controller record went stale (the desktop rotates its
/// self-signed certificate on reinstall, or its IP moves between Wi-Fi and USB
/// tethering), the desktop's "pair" button timed out with "no pairing challenge
/// received" and the only way out was wiping the pairing on both sides.
pub fn begin_pairing_challenge(
    challenge: &SharedChallenge,
    layout: &ReceiverLayout,
    requester: &LanPeer,
    requester_ip: String,
) -> Option<PairingChallenge> {
    if layout.machine_role != "client" {
        return None;
    }
    if requester.machine_role != "server" {
        return None;
    }

    let now = Instant::now();
    let expires_at = now + Duration::from_millis(PAIRING_CODE_TTL_MS);
    let expires_at_ms = now_ms().saturating_add(PAIRING_CODE_TTL_MS);

    let mut guard = challenge.lock().ok()?;
    if let Some(existing) = guard.as_mut() {
        if existing.expires_at > now {
            // Same requester re-asking: keep the live code, but refresh the
            // details we learned about it.
            if existing.requester_id == requester.id {
                if existing.attempts > 0 {
                    existing.code = random_pairing_code();
                    existing.expires_at = expires_at;
                    existing.expires_at_ms = expires_at_ms;
                    existing.attempts = 0;
                }
                existing.requester_ip = requester_ip;
                existing.requester_host = requester.host.clone();
                existing.requester_public_key = requester.transport_public_key.clone();
                existing.requester_protocol_version = requester.protocol_version;
                return Some(existing.clone());
            }
            return None;
        }
    }

    let next = PairingChallenge {
        code: random_pairing_code(),
        requester_id: requester.id.clone(),
        requester_name: requester.name.clone(),
        requester_ip,
        requester_host: requester.host.clone(),
        requester_public_key: requester.transport_public_key.clone(),
        requester_protocol_version: requester.protocol_version,
        expires_at,
        expires_at_ms,
        attempts: 0,
    };
    *guard = Some(next.clone());
    Some(next)
}

/// Validates the code carried by `pair-confirm` and, on success, adopts the
/// desktop's cluster id + pair secret and whitelists it as a controller.
pub fn complete_pairing_from_confirm(
    layout_state: &Arc<Mutex<ReceiverLayout>>,
    challenge: &SharedChallenge,
    requester: &LanPeer,
    code: Option<String>,
    cluster_id: Option<String>,
    pair_secret: Option<String>,
) -> Result<ReceiverLayout, String> {
    let code = code.unwrap_or_default();
    let cluster_id = cluster_id.unwrap_or_default();
    let pair_secret = pair_secret.unwrap_or_default();
    if code.trim().is_empty() || cluster_id.trim().is_empty() || pair_secret.trim().is_empty() {
        return Err("pairing confirm is missing the code or cluster info".into());
    }

    {
        let mut guard = challenge
            .lock()
            .map_err(|_| "pairing challenge lock poisoned".to_string())?;
        let Some(existing) = guard.as_mut() else {
            return Err("no pairing challenge is pending".into());
        };
        if existing.is_expired() {
            *guard = None;
            return Err("the pairing code expired".into());
        }
        if existing.requester_id != requester.id
            || (!existing.requester_public_key.trim().is_empty()
                && existing.requester_public_key != requester.transport_public_key)
        {
            return Err("pairing confirm came from a different requester".into());
        }
        if existing.code != code.trim() {
            existing.attempts = existing.attempts.saturating_add(1);
            if existing.attempts >= PAIRING_MAX_ATTEMPTS {
                *guard = None;
            }
            return Err("incorrect pairing code".into());
        }
        *guard = None;
    }

    let snapshot = {
        let mut layout = layout_state
            .lock()
            .map_err(|_| "receiver layout lock poisoned".to_string())?;
        if layout.machine_role != "client" {
            return Err("only a client can accept a server pairing".into());
        }

        layout.cluster_id = cluster_id.trim().into();
        layout.pair_secret = pair_secret.trim().into();
        layout.paired_controllers = vec![PairedController {
            id: requester.id.clone(),
            name: requester.name.clone(),
            host: requester.host.clone(),
            ip: requester.ip.clone(),
            transport_public_key: requester.transport_public_key.clone(),
            protocol_version: requester.protocol_version,
            cluster_id: layout.cluster_id.clone(),
            paired_at_ms: now_ms(),
        }];
        layout.clone()
    };

    Ok(snapshot)
}

/// The stricter twin of [`can_repair_with_peer`], for the paths that *write* to
/// the stored record.
///
/// `can_repair_with_peer` deliberately also accepts an IP-only match so a user
/// can re-pair a desktop whose hostname they changed. That is fine for deciding
/// whether to show a verification code, but it is not good enough to rewrite
/// stored credentials: two MyKVM instances sharing one PC (the desktop app plus
/// a test controller) have the same IP, and so does every device behind a NAT
/// that happens to reuse the address. A write therefore requires an identity or
/// hostname match, both of which survive a certificate rotation and an IP move.
pub fn can_refresh_controller_identity(controller: &PairedController, peer: &LanPeer) -> bool {
    identity_matches_peer(controller, peer)
        || text_matches(&controller.name, &peer.name)
        || same_host(&controller.host, &peer.host)
        || same_host(&peer.host, &controller.host)
}

pub fn identity_matches_peer(controller: &PairedController, peer: &LanPeer) -> bool {
    (!peer.id.trim().is_empty() && controller.id == peer.id)
        || (!peer.transport_public_key.trim().is_empty()
            && controller.transport_public_key == peer.transport_public_key)
}

fn text_matches(left: &str, right: &str) -> bool {
    let left = left.trim();
    let right = right.trim();
    !left.is_empty() && !right.is_empty() && left.eq_ignore_ascii_case(right)
}

fn same_host(value: &str, host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    value
        .split('/')
        .map(|part| part.trim().to_ascii_lowercase())
        .any(|part| part == host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::LanPeerScreen;

    fn peer(id: &str, role: &str, key: &str) -> LanPeer {
        LanPeer {
            id: id.into(),
            name: "Desktop".into(),
            platform: "windows".into(),
            machine_role: role.into(),
            cluster_id: String::new(),
            pairing_required: false,
            host: "desktop".into(),
            ip: "192.168.1.10".into(),
            transport_port: 47833,
            quic_port: 47834,
            transport_public_key: key.into(),
            protocol_version: 1,
            screen_count: 1,
            input_ready: true,
            upgrading: false,
            screens: Vec::<LanPeerScreen>::new(),
            app_version: "0.1.0".into(),
            last_seen_ms: 0,
        }
    }

    fn unpaired_layout() -> ReceiverLayout {
        ReceiverLayout::new_unpaired()
    }

    #[test]
    fn challenge_is_issued_only_for_a_server_requester() {
        let challenge: SharedChallenge = Arc::new(Mutex::new(None));
        let layout = unpaired_layout();

        assert!(begin_pairing_challenge(&challenge, &layout, &peer("p1", "client", "k1"), "1.2.3.4".into()).is_none());
        assert!(begin_pairing_challenge(&challenge, &layout, &peer("p1", "server", "k1"), "1.2.3.4".into()).is_some());
    }

    /// Re-asking with the same identity keeps the code the user is looking at,
    /// so a retried request does not invalidate the code already on screen.
    #[test]
    fn repeated_request_from_same_peer_keeps_the_code() {
        let challenge: SharedChallenge = Arc::new(Mutex::new(None));
        let layout = unpaired_layout();

        let first = begin_pairing_challenge(&challenge, &layout, &peer("p1", "server", "k1"), "1.2.3.4".into()).unwrap();
        let second = begin_pairing_challenge(&challenge, &layout, &peer("p1", "server", "k1"), "1.2.3.4".into()).unwrap();
        assert_eq!(first.code, second.code);
    }

    /// Re-pairing has to stay possible: once the stored controller record is
    /// stale the desktop's pair button is the user's only way back, and it can
    /// only work if the receiver still mints a challenge while paired.
    #[test]
    fn paired_client_still_accepts_a_repair_request() {
        let challenge: SharedChallenge = Arc::new(Mutex::new(None));
        let mut layout = unpaired_layout();
        layout.cluster_id = "cluster-before-repair".into();
        layout.pair_secret = "secret-before-repair".into();
        layout.paired_controllers.push(crate::packet::PairedController {
            id: "peer-desktop".into(),
            name: "Desktop".into(),
            host: "desktop".into(),
            ip: "192.168.1.10".into(),
            transport_public_key: "stale-key".into(),
            protocol_version: 1,
            cluster_id: "cluster-before-repair".into(),
            paired_at_ms: 0,
        });
        assert!(!layout.pairing_required());

        let requester = peer("peer-desktop", "server", "rotated-key");
        let issued = begin_pairing_challenge(&challenge, &layout, &requester, "192.168.1.10".into());
        assert!(issued.is_some(), "a paired client must still accept a repair request");
        assert_eq!(issued.unwrap().requester_id, "peer-desktop");
    }

    #[test]
    fn confirm_adopts_cluster_credentials_and_whitelists_controller() {
        let challenge: SharedChallenge = Arc::new(Mutex::new(None));
        let layout = Arc::new(Mutex::new(unpaired_layout()));
        let requester = peer("peer-desktop", "server", "pubkey-1");

        let issued = begin_pairing_challenge(&challenge, &layout.lock().unwrap(), &requester, "192.168.1.10".into()).unwrap();

        let done = complete_pairing_from_confirm(
            &layout,
            &challenge,
            &requester,
            Some(issued.code.clone()),
            Some("cluster-xyz".into()),
            Some("secret-xyz".into()),
        )
        .expect("pairing should succeed");

        assert_eq!(done.cluster_id, "cluster-xyz");
        assert_eq!(done.pair_secret, "secret-xyz");
        assert_eq!(done.paired_controllers.len(), 1);
        assert_eq!(done.paired_controllers[0].id, "peer-desktop");
        assert!(!done.pairing_required());
        // The challenge is single-use.
        assert!(challenge.lock().unwrap().is_none());
    }

    #[test]
    fn wrong_code_is_rejected_and_burns_an_attempt() {
        let challenge: SharedChallenge = Arc::new(Mutex::new(None));
        let layout = Arc::new(Mutex::new(unpaired_layout()));
        let requester = peer("peer-desktop", "server", "pubkey-1");
        begin_pairing_challenge(&challenge, &layout.lock().unwrap(), &requester, "192.168.1.10".into()).unwrap();

        let err = complete_pairing_from_confirm(
            &layout,
            &challenge,
            &requester,
            Some("000000".into()),
            Some("cluster-xyz".into()),
            Some("secret-xyz".into()),
        )
        .unwrap_err();
        assert!(err.contains("incorrect"));
        assert_eq!(challenge.lock().unwrap().as_ref().unwrap().attempts, 1);
        assert!(layout.lock().unwrap().paired_controllers.is_empty());
    }
}