package com.mykvm.receiver.core

/**
 * The Rust protocol core (discovery + QUIC input plane).
 *
 * Everything here is a thin JNI shim. Kotlin owns everything Android-shaped
 * (injection, overlay, storage); Rust owns the wire format, so the two sides
 * cannot drift out of sync with the desktop app.
 */
object NativeCore {

    @Volatile
    private var loaded = false

    /** Loads the shared library, returning false when the ABI is missing. */
    @Synchronized
    fun ensureLoaded(): Boolean {
        if (loaded) return true
        return try {
            System.loadLibrary("mykvm_core")
            loaded = true
            true
        } catch (error: UnsatisfiedLinkError) {
            false
        }
    }

    /**
     * Starts the receiver. Returns a JSON object:
     * `{"ok":true,"quicPort":..,"discoveryPort":..,"peerId":"..",...}`
     * or `{"ok":false,"error":".."}`.
     */
    external fun nativeStart(configJson: String): String

    external fun nativeStop()

    /**
     * Blocks up to [timeoutMs] for the next event, returning a JSON object or
     * null on timeout. The underlying channel is unbounded, so a pending input
     * event wakes the call immediately.
     */
    external fun nativePoll(timeoutMs: Long): String?

    external fun nativeIsPaired(): Boolean

    /** Replaces the in-memory layout JSON (used when unpairing). */
    external fun nativeSetLayout(layoutJson: String): String

    external fun nativeVersion(): String
}