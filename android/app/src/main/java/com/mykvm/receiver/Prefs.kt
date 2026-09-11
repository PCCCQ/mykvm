package com.mykvm.receiver

import android.content.Context
import android.os.Build
import java.security.SecureRandom

/**
 * Persistent receiver settings.
 *
 * [stableId] deliberately never contains the device IP: the desktop keys its
 * saved devices off the id we announce, and a phone changes address whenever it
 * moves between Wi-Fi and USB tethering.
 */
class Prefs(context: Context) {

    private val store = context.getSharedPreferences("mykvm-receiver", Context.MODE_PRIVATE)

    var stableId: String
        get() {
            val existing = store.getString(KEY_STABLE_ID, null)
            if (existing != null) return existing
            val generated = "android-" + randomHex(8)
            store.edit().putString(KEY_STABLE_ID, generated).apply()
            return generated
        }
        set(value) = store.edit().putString(KEY_STABLE_ID, value).apply()

    var deviceName: String
        get() = store.getString(KEY_DEVICE_NAME, null)
            ?: Build.MODEL?.takeIf { it.isNotBlank() } ?: "Android"
        set(value) = store.edit().putString(KEY_DEVICE_NAME, value).apply()

    /** The exact `layout` object the Rust core handed back when pairing completed. */
    var layoutJson: String?
        get() = store.getString(KEY_LAYOUT, null)
        set(value) {
            val editor = store.edit()
            if (value == null) editor.remove(KEY_LAYOUT) else editor.putString(KEY_LAYOUT, value)
            editor.apply()
        }

    /** Whether we were running when the process died, so we can auto-restart. */
    /**
     * How input is delivered: native mouse events or synthetic touch.
     * Defaults to mouse mode; see [com.mykvm.receiver.input.InputMode].
     */
    var inputMode: com.mykvm.receiver.input.InputMode
        get() = com.mykvm.receiver.input.InputMode.fromKey(store.getString(KEY_INPUT_MODE, null))
        set(value) = store.edit().putString(KEY_INPUT_MODE, value.key).apply()

    /**
     * On-screen pointer size in dp. The default matches the old hardcoded
     * 48px on a 340dpi panel, rounded to a friendlier number.
     */
    /**
     * Suppress the tablet's own IME while the receiver runs, so injected keys
     * reach the focused app instead of being composed as pinyin.
     */
    /**
     * Write clipboard text received from the desktop to the system clipboard.
     * Only this direction: Android 10+ stops a background service from
     * *reading* the clipboard, so tablet -> desktop is not implemented.
     */
    var clipboardSync: Boolean
        get() = store.getBoolean(KEY_CLIPBOARD_SYNC, true)
        set(value) = store.edit().putBoolean(KEY_CLIPBOARD_SYNC, value).apply()

    var keyboardPassthrough: Boolean
        get() = store.getBoolean(KEY_KEYBOARD_PASSTHROUGH, true)
        set(value) = store.edit().putBoolean(KEY_KEYBOARD_PASSTHROUGH, value).apply()

    /**
     * The IME that was active before [keyboardPassthrough] replaced it, kept
     * so it can be restored even after a crash.
     */
    /** `enabled_input_methods` as it was before suppression. */
    var suppressedEnabledIms: String?
        get() = store.getString(KEY_SUPPRESSED_ENABLED_IMS, null)
        set(value) {
            val editor = store.edit()
            if (value == null) editor.remove(KEY_SUPPRESSED_ENABLED_IMS)
            else editor.putString(KEY_SUPPRESSED_ENABLED_IMS, value)
            editor.apply()
        }

    var suppressedIme: String?
        get() = store.getString(KEY_SUPPRESSED_IME, null)
        set(value) {
            val editor = store.edit()
            if (value == null) editor.remove(KEY_SUPPRESSED_IME)
            else editor.putString(KEY_SUPPRESSED_IME, value)
            editor.apply()
        }

    var cursorSizeDp: Int
        get() = store.getInt(KEY_CURSOR_SIZE_DP, DEFAULT_CURSOR_SIZE_DP)
        set(value) = store.edit().putInt(KEY_CURSOR_SIZE_DP, value).apply()

    var shouldRun: Boolean
        get() = store.getBoolean(KEY_SHOULD_RUN, false)
        set(value) = store.edit().putBoolean(KEY_SHOULD_RUN, value).apply()

    /**
     * Display name of the paired controller.
     *
     * Derived from the persisted layout rather than stored separately: the
     * layout document is the single source of truth, and a duplicate field
     * could drift from it (or be missing for a device paired by an older build).
     */
    val controllerName: String?
        get() {
            val raw = layoutJson ?: return null
            return try {
                val controllers = org.json.JSONObject(raw)
                    .optJSONArray("pairedControllers") ?: return null
                if (controllers.length() == 0) return null
                val first = controllers.optJSONObject(0) ?: return null
                first.optString("name").takeIf { it.isNotBlank() }
                    ?: first.optString("id").takeIf { it.isNotBlank() }
            } catch (error: Throwable) {
                null
            }
        }

    val isPaired: Boolean get() = !layoutJson.isNullOrBlank()

    fun unpair() {
        layoutJson = null
    }

    private fun randomHex(byteCount: Int): String {
        val bytes = ByteArray(byteCount)
        SecureRandom().nextBytes(bytes)
        return bytes.joinToString("") { "%02x".format(it) }
    }

    private companion object {
        const val KEY_STABLE_ID = "stable-id"
        const val KEY_DEVICE_NAME = "device-name"
        const val KEY_LAYOUT = "layout-json"
        const val KEY_CONTROLLER_NAME = "controller-name"
        const val KEY_SHOULD_RUN = "should-run"
        const val KEY_INPUT_MODE = "input-mode"
        const val KEY_CURSOR_SIZE_DP = "cursor-size-dp"
        const val KEY_KEYBOARD_PASSTHROUGH = "keyboard-passthrough"
        const val KEY_CLIPBOARD_SYNC = "clipboard-sync"
        const val KEY_SUPPRESSED_IME = "suppressed-ime"
        const val KEY_SUPPRESSED_ENABLED_IMS = "suppressed-enabled-ims"
        const val DEFAULT_CURSOR_SIZE_DP = 28
    }
}
