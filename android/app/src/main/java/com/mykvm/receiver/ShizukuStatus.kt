package com.mykvm.receiver

import android.content.Context
import android.content.pm.PackageManager
import android.provider.Settings
import rikka.shizuku.Shizuku

/**
 * Single source of truth for the Shizuku / overlay status shown in the UI.
 *
 * Kept out of [KvmService] because the two are independent: the user has to be
 * able to see (and fix) "Shizuku is running but not authorised" *before*
 * starting the receiver, and the service may not be running at all yet.
 */
object ShizukuStatus {

    fun isRunning(): Boolean = try {
        Shizuku.pingBinder()
    } catch (error: Throwable) {
        false
    }

    fun hasPermission(): Boolean = try {
        Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
    } catch (error: Throwable) {
        false
    }

    /**
     * Re-reads every permission-derived flag into [ReceiverState].
     *
     * Cheap enough to call on every resume and on a slow timer: `pingBinder`
     * and `checkSelfPermission` are local checks, not binder round trips.
     */
    fun refresh(context: Context) {
        val running = isRunning()
        val granted = hasPermission()
        ReceiverState.update {
            it.copy(
                shizukuRunning = running,
                injectionReady = running && granted,
                overlayGranted = Settings.canDrawOverlays(context),
            )
        }
    }

    fun requestPermission(requestCode: Int): Boolean = try {
        if (!isRunning()) {
            false
        } else {
            Shizuku.requestPermission(requestCode)
            true
        }
    } catch (error: Throwable) {
        false
    }
}