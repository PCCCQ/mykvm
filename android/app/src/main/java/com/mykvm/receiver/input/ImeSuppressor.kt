package com.mykvm.receiver.input

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.provider.Settings
import android.util.Log
import com.mykvm.receiver.Prefs

/**
 * Takes the tablet's soft keyboard out of the input path while the receiver runs.
 *
 * Why this is needed: when a soft keyboard (IME) is the active input method,
 * Android delivers injected key events to the IME first. On the test tablet the
 * Sogou IME turns them into pinyin composition -- the candidate bar fills up and
 * nothing ever reaches the app. It is not specific to us: `adb shell input text`
 * is swallowed the same way. Verified on device that with the IME taken out of
 * the way the same injected events type correctly.
 *
 * Two settings have to change together:
 *
 *  - `ENABLED_INPUT_METHODS` cleared is what actually removes the IME from the
 *    key path (this is what `ime disable` does). Pointing only
 *    `DEFAULT_INPUT_METHOD` at a dead id is NOT enough: the system still treats
 *    the keys as belonging to an IME session and drops them -- measured, our
 *    injected keys were dropped while `adb shell input text` still worked.
 *  - `DEFAULT_INPUT_METHOD` moved off the real IME so the system does not try
 *    to re-activate the component that is no longer enabled.
 *
 * Both originals are remembered and restored when the receiver stops, and again
 * on the next app start, so a crash cannot leave the tablet without a keyboard.
 *
 * Requires WRITE_SECURE_SETTINGS, which only adb can grant:
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

    private fun read(context: Context, key: String): String? = try {
        Settings.Secure.getString(context.contentResolver, key)
    } catch (error: Throwable) {
        null
    }

    private fun write(context: Context, key: String, value: String): Boolean = try {
        Settings.Secure.putString(context.contentResolver, key, value)
    } catch (error: Throwable) {
        Log.w(TAG, "cannot write $key: ${error.message}")
        false
    }

    /**
     * Remembers the current IME settings and takes the IME out of the key path.
     *
     * Idempotent: the saved values are only written the first time, so a service
     * restart cannot overwrite them with the placeholder.
     */
    fun suppress(context: Context, prefs: Prefs): Boolean {
        if (!hasPermission(context)) return false

        val active = read(context, Settings.Secure.DEFAULT_INPUT_METHOD)
        if (active == DISABLED_IME) return true
        if (active.isNullOrBlank()) return false

        if (prefs.suppressedIme == null) {
            prefs.suppressedIme = active
        }
        if (prefs.suppressedEnabledIms == null) {
            prefs.suppressedEnabledIms =
                read(context, Settings.Secure.ENABLED_INPUT_METHODS) ?: ""
        }

        val ok = write(context, Settings.Secure.ENABLED_INPUT_METHODS, "") and
            write(context, Settings.Secure.DEFAULT_INPUT_METHOD, DISABLED_IME)
        if (ok) Log.i(TAG, "suppressed IME $active")
        return ok
    }

    /**
     * Puts the saved settings back. The saved values are cleared first so a
     * failure does not leave us retrying an IME that has since been removed.
     */
    fun restore(context: Context, prefs: Prefs): Boolean {
        val saved = prefs.suppressedIme
        val savedEnabled = prefs.suppressedEnabledIms
        if (saved == null && savedEnabled == null) return true

        prefs.suppressedIme = null
        prefs.suppressedEnabledIms = null

        if (!hasPermission(context)) return false

        var ok = true
        if (!saved.isNullOrBlank()) {
            ok = write(context, Settings.Secure.DEFAULT_INPUT_METHOD, saved)
        }
        if (!savedEnabled.isNullOrEmpty()) {
            ok = write(context, Settings.Secure.ENABLED_INPUT_METHODS, savedEnabled) and ok
        }
        if (ok) Log.i(TAG, "restored IME $saved")
        return ok
    }
}