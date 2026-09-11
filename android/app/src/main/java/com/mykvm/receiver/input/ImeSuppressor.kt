package com.mykvm.receiver.input

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.provider.Settings
import android.util.Log
import android.view.inputmethod.InputMethodManager
import com.mykvm.receiver.Prefs

/**
 * Takes the tablet's soft keyboard out of the input path while the desktop is
 * driving it.
 *
 * Why this is needed: when a soft keyboard (IME) is the active input method,
 * Android delivers injected key events to the IME first. On the test tablet the
 * Sogou IME turns them into pinyin composition -- the candidate bar fills up and
 * nothing reaches the app. It is not specific to us: `adb shell input text` is
 * swallowed the same way. With the IME out of the path the same injected events
 * type correctly.
 *
 * ONLY `DEFAULT_INPUT_METHOD` is moved. An earlier version also emptied
 * `ENABLED_INPUT_METHODS`, which was a mistake: it left the device with no input
 * method at all, so the user could neither switch keyboards nor type, and a
 * crash before the restore made that permanent. Leaving the enabled list alone
 * means the user's keyboards stay installed and selectable, and a failure can
 * always be undone from the tablet's settings.
 *
 * Requires WRITE_SECURE_SETTINGS, granted once over adb:
 *
 *   adb shell pm grant com.mykvm.receiver android.permission.WRITE_SECURE_SETTINGS
 */
object ImeSuppressor {

    private const val TAG = "MyKvmIme"

    /** Syntactically valid, but no such component exists. */
    private const val DISABLED_IME = "mykvm/no-ime"

    fun hasPermission(context: Context): Boolean = try {
        context.checkSelfPermission(Manifest.permission.WRITE_SECURE_SETTINGS) ==
            PackageManager.PERMISSION_GRANTED
    } catch (error: Throwable) {
        false
    }

    private fun readDefault(context: Context): String? = try {
        Settings.Secure.getString(
            context.contentResolver,
            Settings.Secure.DEFAULT_INPUT_METHOD,
        )
    } catch (error: Throwable) {
        null
    }

    private fun writeDefault(context: Context, value: String): Boolean = try {
        Settings.Secure.putString(
            context.contentResolver,
            Settings.Secure.DEFAULT_INPUT_METHOD,
            value,
        )
    } catch (error: Throwable) {
        Log.w(TAG, "cannot write DEFAULT_INPUT_METHOD: ${error.message}")
        false
    }

    /**
     * The IME ids the user currently has enabled.
     *
     * Taken from InputMethodManager rather than by parsing
     * `Settings.Secure.ENABLED_INPUT_METHODS`: that read came back empty on the
     * test tablet even though the setting held three IMEs, which left the
     * crash-recovery path with nothing to restore.
     */
    private fun enabledImeIds(context: Context): List<String> = try {
        context.getSystemService(InputMethodManager::class.java)
            ?.enabledInputMethodList
            ?.map { it.id }
            .orEmpty()
    } catch (error: Throwable) {
        Log.w(TAG, "cannot list enabled input methods: ${error.message}")
        emptyList()
    }

    /**
     * Moves the default input method off the real IME.
     *
     * Idempotent: the saved value is only written the first time, so a service
     * restart cannot overwrite it with the placeholder.
     */
    fun suppress(context: Context, prefs: Prefs): Boolean {
        if (!hasPermission(context)) return false

        val active = readDefault(context)
        if (active == DISABLED_IME) return true
        // Nothing to take over when there is no default IME; saving an empty
        // value here is what made an earlier version unrecoverable.
        if (active.isNullOrBlank()) return false

        if (prefs.suppressedIme == null) {
            prefs.suppressedIme = active
        }

        val ok = writeDefault(context, DISABLED_IME)
        if (ok) Log.i(TAG, "keyboard passthrough on (was $active)")
        return ok
    }

    /**
     * Puts a working IME back.
     *
     * The saved value is never trusted blindly: if it is missing, empty, or no
     * longer installed, the first enabled IME is used instead. The one thing
     * this must never do is leave the tablet without a keyboard.
     */
    fun restore(context: Context, prefs: Prefs): Boolean {
        val saved = prefs.suppressedIme
        prefs.suppressedIme = null

        if (!hasPermission(context)) return false

        val enabled = enabledImeIds(context)
        val target = when {
            saved != null && enabled.contains(saved) -> saved
            enabled.isNotEmpty() -> enabled.first()
            else -> null
        }

        if (target == null) {
            Log.w(TAG, "no enabled input method to restore")
            return false
        }

        val ok = writeDefault(context, target)
        if (ok) Log.i(TAG, "keyboard passthrough off (restored $target)")
        return ok
    }

    /**
     * Called on app start. If a previous run died mid-session the default still
     * points at the placeholder, so put a real IME back.
     */
    fun repairIfNeeded(context: Context, prefs: Prefs): Boolean {
        val active = readDefault(context)
        if (active != DISABLED_IME && !active.isNullOrBlank()) return true
        return restore(context, prefs)
    }
}