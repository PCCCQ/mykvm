//! Receiver-side state that must stay wire-compatible with the desktop app.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::packet::PairedController;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// 6 digit code, matching the desktop's `random_pairing_code`.
pub fn random_pairing_code() -> String {
    let rng = SystemRandom::new();
    let mut bytes = [0_u8; 4];
    if rng.fill(&mut bytes).is_err() {
        bytes = now_ms().to_le_bytes()[..4].try_into().unwrap_or([0; 4]);
    }
    format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000)
}

/// The pairing-relevant half of the desktop's `LayoutState`.
///
/// The desktop persists a whole layout document; the receiver only needs the
/// fields that decide whether an inbound input packet is authorised:
/// `machine_role`, `cluster_id`, `pair_secret` and `paired_controllers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverLayout {
    #[serde(default = "default_machine_role")]
    pub machine_role: String,
    #[serde(default)]
    pub cluster_id: String,
    #[serde(default)]
    pub pair_secret: String,
    #[serde(default)]
    pub paired_controllers: Vec<PairedController>,
}

fn default_machine_role() -> String {
    "client".into()
}

impl Default for ReceiverLayout {
    fn default() -> Self {
        Self::new_unpaired()
    }
}

impl ReceiverLayout {
    pub fn new_unpaired() -> Self {
        Self {
            machine_role: "client".into(),
            cluster_id: String::new(),
            pair_secret: String::new(),
            paired_controllers: Vec::new(),
        }
    }

    /// Mirrors the desktop's `pairing_required`.
    pub fn pairing_required(&self) -> bool {
        self.machine_role == "client" && self.paired_controllers.is_empty()
    }

    /// Mirrors the desktop's `advertised_cluster_id`: an unpaired client
    /// announces an empty cluster so only servers will talk to it.
    pub fn advertised_cluster_id(&self) -> String {
        if self.pairing_required() {
            String::new()
        } else {
            self.cluster_id.clone()
        }
    }

    /// Whether the desktop is allowed to place a cursor on this screen.
    pub fn input_ready(&self) -> bool {
        !self.pairing_required() && !self.cluster_id.trim().is_empty()
    }

    pub fn is_paired(&self) -> bool {
        !self.paired_controllers.is_empty() && !self.cluster_id.trim().is_empty()
    }
}

/// Authority to inject, mirroring `input::packet_authorized_fields`.
pub fn input_authorized(
    layout: &ReceiverLayout,
    cluster_id: &str,
    pair_secret: &str,
    origin_transport_public_key: &str,
    origin_device_id: &str,
) -> bool {
    if layout.cluster_id.trim().is_empty() || layout.pair_secret.trim().is_empty() {
        return false;
    }
    if cluster_id != layout.cluster_id || pair_secret != layout.pair_secret {
        return false;
    }

    layout.paired_controllers.iter().any(|controller| {
        (!origin_transport_public_key.trim().is_empty()
            && controller.transport_public_key == origin_transport_public_key)
            || (!origin_device_id.trim().is_empty() && controller.id == origin_device_id)
    })
}

/// Caches which origin addresses proved the pairing secret recently, so the
/// steady-state credential-less datagrams (which omit the ~0.5KB pairing block)
/// can still be trusted. Mirrors `input::authorized_input_origins`.
#[derive(Debug, Default)]
pub struct OriginCache {
    entries: std::collections::HashMap<std::net::SocketAddr, Instant>,
}

/// Mirrors `INPUT_ORIGIN_CACHE_TTL` in the desktop app.
pub const INPUT_ORIGIN_CACHE_TTL: Duration = Duration::from_secs(30);

impl OriginCache {
    pub fn remember(&mut self, source: std::net::SocketAddr) {
        let now = Instant::now();
        self.entries.retain(|_, at| now.duration_since(*at) < INPUT_ORIGIN_CACHE_TTL);
        self.entries.insert(source, now);
    }

    pub fn is_fresh(&self, source: std::net::SocketAddr) -> bool {
        self.entries
            .get(&source)
            .is_some_and(|at| at.elapsed() < INPUT_ORIGIN_CACHE_TTL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpaired_client_advertises_nothing_and_cannot_receive() {
        let layout = ReceiverLayout::new_unpaired();
        assert!(layout.pairing_required());
        assert_eq!(layout.advertised_cluster_id(), "");
        assert!(!layout.input_ready());
    }

    #[test]
    fn paired_layout_becomes_an_input_target() {
        let mut layout = ReceiverLayout::new_unpaired();
        layout.cluster_id = "cluster-1".into();
        layout.pair_secret = "secret-1".into();
        layout.paired_controllers.push(PairedController {
            id: "peer-desktop".into(),
            name: "Desktop".into(),
            host: "desktop".into(),
            ip: "192.168.1.10".into(),
            transport_public_key: "key-1".into(),
            protocol_version: 1,
            cluster_id: "cluster-1".into(),
            paired_at_ms: 0,
        });

        assert!(!layout.pairing_required());
        assert_eq!(layout.advertised_cluster_id(), "cluster-1");
        assert!(layout.input_ready());
    }

    #[test]
    fn authorization_requires_matching_cluster_secret_and_a_known_origin() {
        let mut layout = ReceiverLayout::new_unpaired();
        layout.cluster_id = "cluster-1".into();
        layout.pair_secret = "secret-1".into();
        layout.paired_controllers.push(PairedController {
            id: "peer-desktop".into(),
            name: "Desktop".into(),
            host: "desktop".into(),
            ip: "192.168.1.10".into(),
            transport_public_key: "key-1".into(),
            protocol_version: 1,
            cluster_id: "cluster-1".into(),
            paired_at_ms: 0,
        });

        assert!(input_authorized(&layout, "cluster-1", "secret-1", "key-1", ""));
        assert!(input_authorized(&layout, "cluster-1", "secret-1", "", "peer-desktop"));
        // Wrong secret, wrong cluster, unknown origin.
        assert!(!input_authorized(&layout, "cluster-1", "nope", "key-1", ""));
        assert!(!input_authorized(&layout, "other", "secret-1", "key-1", ""));
        assert!(!input_authorized(&layout, "cluster-1", "secret-1", "key-2", "peer-other"));
    }

    #[test]
    fn origin_cache_is_fresh_only_within_ttl() {
        let mut cache = OriginCache::default();
        let addr: std::net::SocketAddr = "192.168.1.10:47834".parse().unwrap();
        assert!(!cache.is_fresh(addr));
        cache.remember(addr);
        assert!(cache.is_fresh(addr));
        // A different source address is not covered by the cached authorization.
        assert!(!cache.is_fresh("192.168.1.11:47834".parse().unwrap()));
    }
}
