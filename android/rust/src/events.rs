//! The single event stream the Kotlin layer consumes.

use crate::packet::InputEvent;
use crate::state::ReceiverLayout;

#[derive(Debug, Clone)]
pub enum ReceiverEvent {
    /// A decoded input event that passed authorization. This is the hot path:
    /// one of these per mouse move.
    Input(InputEvent),
    /// Clipboard text copied on the desktop; write it to the system clipboard.
    ClipboardText(String),
    /// A desktop asked to pair; show `code` to the user.
    PairingRequested {
        code: String,
        requester_name: String,
        requester_ip: String,
        expires_at_ms: u64,
    },
    /// The pending pairing code expired or was superseded.
    PairingCleared,
    /// Pairing succeeded; the caller should persist the layout.
    Paired {
        controller_id: String,
        controller_name: String,
        /// The updated layout, so Kotlin can persist the pairing.
        layout: ReceiverLayout,
    },
    PairingFailed {
        reason: String,
    },
    /// Online controllers, so the UI can show "desktop connected".
    PeerPresence { peer_ids: Vec<String> },
}

pub type EventSink = std::sync::Arc<dyn Fn(ReceiverEvent) + Send + Sync + 'static>;
