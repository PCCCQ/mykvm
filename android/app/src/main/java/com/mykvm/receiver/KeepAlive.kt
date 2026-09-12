package com.mykvm.receiver

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import androidx.core.content.ContextCompat
import com.mykvm.receiver.diag.Diag

/**
 * Brings the receiver back after the system kills it.
 *
 * A foreground service is not a guarantee on aggressive ROMs (Lenovo/Motorola
 * "battery optimization", Samsung's sleeping apps, MIUI's autostart gate): the
 * process can be frozen or killed even while it holds a wake lock. The tablet
 * is usually asleep and unattended when that happens, which is exactly when the
 * desktop notices the connection is gone.
 *
 * Two cheap nets, both best-effort:
 *   * the user is offered the standard battery-optimisation exemption, and
 *   * an `setAndAllowWhileIdle` alarm fires roughly every nine minutes and
 *     re-starts the service if it should be running.
 *
 * The alarm is deliberately non-exact: it does not need the
 * SCHEDULE_EXACT_ALARM permission, and nine minutes is far below the interval
 * at which a dropped desktop connection becomes noticeable.
 */
object KeepAlive {

    const val ACTION_KEEP_ALIVE = "com.mykvm.receiver.KEEP_ALIVE"
    private const val REQUEST_CODE = 4783
    private const val INTERVAL_MS = 9 * 60 * 1000L

    fun schedule(context: Context) {
        val alarm = context.getSystemService(AlarmManager::class.java) ?: return
        try {
            alarm.setAndAllowWhileIdle(
                AlarmManager.ELAPSED_REALTIME_WAKEUP,
                SystemClock.elapsedRealtime() + INTERVAL_MS,
                pendingIntent(context),
            )
        } catch (error: Throwable) {
            Diag.warn("cannot schedule the keep-alive alarm", error)
        }
    }

    fun cancel(context: Context) {
        val alarm = context.getSystemService(AlarmManager::class.java) ?: return
        try {
            alarm.cancel(pendingIntent(context))
        } catch (error: Throwable) {
            Diag.warn("cannot cancel the keep-alive alarm", error)
        }
    }

    private fun pendingIntent(context: Context): PendingIntent = PendingIntent.getBroadcast(
        context,
        REQUEST_CODE,
        Intent(context, KeepAliveReceiver::class.java).setAction(ACTION_KEEP_ALIVE),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    /** Whether the OS currently lets us run without being frozen. */
    fun isExemptFromBatteryOptimizations(context: Context): Boolean {
        val power = context.getSystemService(android.os.PowerManager::class.java) ?: return false
        return try {
            power.isIgnoringBatteryOptimizations(context.packageName)
        } catch (error: Throwable) {
            false
        }
    }
}

/**
 * Restarts the receiver after a reboot or a keep-alive alarm.
 *
 * Kept separate from [KvmService] so the system has something to deliver the
 * broadcast to even when our process has been killed outright.
 */
class KeepAliveReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        if (action != KeepAlive.ACTION_KEEP_ALIVE && action != Intent.ACTION_BOOT_COMPLETED) {
            return
        }

        val prefs = Prefs(context)
        if (!prefs.shouldRun) {
            Diag.info("$action ignored: the receiver was stopped by the user")
            return
        }

        Diag.info("$action: restarting the receiver")
        try {
            ContextCompat.startForegroundService(
                context,
                Intent(context, KvmService::class.java),
            )
        } catch (error: Throwable) {
            // Background-start restrictions can refuse this; the next alarm or a
            // user tap still gets us back.
            Diag.warn("cannot restart the receiver from $action", error)
        }
        KeepAlive.schedule(context)
    }
}
