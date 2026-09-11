//! JNI surface for the Kotlin app.
//!
//! Kotlin owns everything Android-shaped (injection, overlay, storage); this
//! library owns everything protocol-shaped. The boundary is deliberately JSON
//! rather than a rich JNI object graph: the input hot path is a single
//! `nativePoll` returning a small JSON string, and events are rare enough that
//! the allocation does not matter.
//!
//! `nativePoll` blocks on an unbounded channel with a timeout. Because the
//! channel is unbounded, a mouse move wakes the call immediately -- the timeout
//! only bounds how long a shutdown request has to wait for the poll to return.

pub mod discovery;
mod events;
pub mod packet;
mod pairing;
// Verbatim copy of src-tauri/src/quic_transport.rs so the two stay diffable.
// A receiver only ever accepts connections, so the send-side helpers inside it
// are unused here -- that is expected, not a defect.
#[allow(dead_code)]
pub mod quic_transport;
mod receiver;
mod state;

use std::sync::mpsc::Receiver as MpscReceiver;
use std::sync::Mutex;
#[cfg(target_os = "android")]
use std::sync::OnceLock;
use std::time::Duration;

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use events::ReceiverEvent;
use receiver::{Receiver, ReceiverStartup};
use state::ReceiverLayout;

/// Only one receiver runs per process; the service owns its lifecycle.
static RECEIVER: Mutex<Option<Receiver>> = Mutex::new(None);
/// Kept separate from `RECEIVER` so a blocking poll never holds the lock that
/// `nativeStop` needs.
static EVENTS: Mutex<Option<MpscReceiver<ReceiverEvent>>> = Mutex::new(None);

/// Android-only: pipes the log facade into logcat. Elsewhere this is a no-op
/// so the protocol crate still builds (and its tests still run) on a host.
#[cfg(target_os = "android")]
fn logger_init() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("mykvm-core"),
        );
    });
}

#[cfg(not(target_os = "android"))]
fn logger_init() {}

fn json_error(message: &str) -> String {
    serde_json::json!({ "ok": false, "error": message }).to_string()
}

fn json_ok() -> String {
    serde_json::json!({ "ok": true }).to_string()
}

fn to_jstring(env: &mut JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(string) => string.into_raw(),
        Err(_) => {
            // Returning a null jstring is the only remaining option; Kotlin
            // treats it as "no event".
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mykvm_receiver_core_NativeCore_nativeStart<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    config: JString<'local>,
) -> jstring {
    logger_init();

    let config: String = match env.get_string(&config) {
        Ok(value) => value.into(),
        Err(error) => {
            let message = format!("invalid start config: {error}");
            return to_jstring(&mut env, &json_error(&message));
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&config) {
        Ok(value) => value,
        Err(error) => {
            let message = format!("start config is not valid JSON: {error}");
            return to_jstring(&mut env, &json_error(&message));
        }
    };

    let string = |key: &str| -> Option<String> {
        parsed
            .get(key)
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
    };
    let number = |key: &str, fallback: i64| -> i64 {
        parsed
            .get(key)
            .and_then(|value| value.as_i64())
            .unwrap_or(fallback)
    };

    let Some(device_name) = string("deviceName") else {
        return to_jstring(&mut env, &json_error("deviceName is required"));
    };
    let Some(stable_id) = string("stableId") else {
        return to_jstring(&mut env, &json_error("stableId is required"));
    };
    let Some(identity_dir) = string("identityDir") else {
        return to_jstring(&mut env, &json_error("identityDir is required"));
    };

    let initial_layout = parsed
        .get("layout")
        .and_then(|value| serde_json::from_value::<ReceiverLayout>(value.clone()).ok())
        .unwrap_or_default();

    let startup = ReceiverStartup {
        device_name,
        stable_id,
        app_version: string("appVersion").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
        screen_id: string("screenId").unwrap_or_else(|| "android-screen-1".into()),
        screen_width: number("screenWidth", 1080) as i32,
        screen_height: number("screenHeight", 2400) as i32,
        discovery_port: number("discoveryPort", packet::DISCOVERY_PORT as i64) as u16,
        quic_port: number("quicPort", (packet::DISCOVERY_PORT + 1) as i64) as u16,
        identity_dir: identity_dir.into(),
        initial_layout,
    };

    // Tear down any previous run before starting a new one.
    if let Ok(mut slot) = RECEIVER.lock() {
        if let Some(mut previous) = slot.take() {
            previous.stop();
        }
    }
    if let Ok(mut slot) = EVENTS.lock() {
        *slot = None;
    }

    match Receiver::start(startup) {
        Ok(mut started) => {
            // Drain nothing yet; just park the receiver and hand the event
            // stream to the poll side.
            let response = serde_json::json!({
                "ok": true,
                "quicPort": started.config.quic_port,
                "discoveryPort": started.config.discovery_port,
                "peerId": discovery::local_peer_id(&started.config.stable_id),
                "transportPublicKey": started.config.transport_public_key,
                "paired": started.is_paired(),
            })
            .to_string();

            // Split the event stream out of the receiver so polling never
            // contends with control calls.
            let events = started.take_events();
            if let Ok(mut slot) = EVENTS.lock() {
                *slot = events;
            }
            if let Ok(mut slot) = RECEIVER.lock() {
                *slot = Some(started);
            }
            to_jstring(&mut env, &response)
        }
        Err(error) => to_jstring(&mut env, &json_error(&error)),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mykvm_receiver_core_NativeCore_nativeStop<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    if let Ok(mut slot) = EVENTS.lock() {
        *slot = None;
    }
    if let Ok(mut slot) = RECEIVER.lock() {
        if let Some(mut receiver) = slot.take() {
            receiver.stop();
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mykvm_receiver_core_NativeCore_nativePoll<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    timeout_ms: jni::sys::jlong,
) -> jstring {
    let timeout = Duration::from_millis(timeout_ms.max(0) as u64);

    let event = {
        let Ok(mut slot) = EVENTS.lock() else {
            return std::ptr::null_mut();
        };
        let Some(events) = slot.as_mut() else {
            return std::ptr::null_mut();
        };
        match events.recv_timeout(timeout) {
            Ok(event) => event,
            Err(_) => return std::ptr::null_mut(),
        }
    };

    let json = match event {
        ReceiverEvent::Input(event) => serde_json::json!({
            "type": "input",
            "event": event,
        }),
        ReceiverEvent::PairingRequested {
            code,
            requester_name,
            requester_ip,
            expires_at_ms,
        } => serde_json::json!({
            "type": "pairingRequested",
            "code": code,
            "requesterName": requester_name,
            "requesterIp": requester_ip,
            "expiresAtMs": expires_at_ms,
        }),
        ReceiverEvent::ClipboardText(text) => serde_json::json!({
            "type": "clipboardText",
            "text": text,
        }),
        ReceiverEvent::PairingCleared => serde_json::json!({ "type": "pairingCleared" }),
        ReceiverEvent::Paired {
            controller_id,
            controller_name,
            layout,
        } => serde_json::json!({
            "type": "paired",
            "controllerId": controller_id,
            "controllerName": controller_name,
            "layout": layout,
        }),
        ReceiverEvent::PairingFailed { reason } => serde_json::json!({
            "type": "pairingFailed",
            "reason": reason,
        }),
        ReceiverEvent::PeerPresence { peer_ids } => serde_json::json!({
            "type": "peerPresence",
            "peerIds": peer_ids,
        }),
    };

    to_jstring(&mut env, &json.to_string())
}

#[no_mangle]
pub extern "system" fn Java_com_mykvm_receiver_core_NativeCore_nativeIsPaired<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jni::sys::jboolean {
    let paired = RECEIVER
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(Receiver::is_paired))
        .unwrap_or(false);
    if paired {
        jni::sys::JNI_TRUE
    } else {
        jni::sys::JNI_FALSE
    }
}

/// Replaces the in-memory layout, e.g. when the user unpairs.
#[no_mangle]
pub extern "system" fn Java_com_mykvm_receiver_core_NativeCore_nativeSetLayout<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    layout_json: JString<'local>,
) -> jstring {
    let raw: String = match env.get_string(&layout_json) {
        Ok(value) => value.into(),
        Err(error) => {
            let message = format!("invalid layout: {error}");
            return to_jstring(&mut env, &json_error(&message));
        }
    };

    let layout: ReceiverLayout = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            let message = format!("layout is not valid JSON: {error}");
            return to_jstring(&mut env, &json_error(&message));
        }
    };

    let Ok(slot) = RECEIVER.lock() else {
        return to_jstring(&mut env, &json_error("receiver lock poisoned"));
    };
    let Some(receiver) = slot.as_ref() else {
        return to_jstring(&mut env, &json_error("receiver is not running"));
    };
    receiver.set_layout(layout);
    to_jstring(&mut env, &json_ok())
}

#[no_mangle]
pub extern "system" fn Java_com_mykvm_receiver_core_NativeCore_nativeVersion<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    to_jstring(&mut env, env!("CARGO_PKG_VERSION"))
}
