package com.mykvm.receiver

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.os.Build

class MyKvmApp : Application() {

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()

        // Seed the UI state so the first frame shows the real device name
        // instead of a blank string; the service only fills it in once it runs.
        val prefs = Prefs(this)
        ReceiverState.update {
            it.copy(
                deviceName = prefs.deviceName,
                inputMode = prefs.inputMode,
                cursorSizeDp = prefs.cursorSizeDp,
                keyboardPassthrough = prefs.keyboardPassthrough,
                clipboardSync = prefs.clipboardSync,
                imePermission = com.mykvm.receiver.input.ImeSuppressor.hasPermission(this),
                paired = prefs.isPaired,
                connectedController = prefs.controllerName,
            )
        }
        // Recover from a previous run that died while keyboard passthrough was on:
        // the default input method would still point at the placeholder.
        com.mykvm.receiver.input.ImeSuppressor.repairIfNeeded(this, prefs)

        ShizukuStatus.refresh(this)
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            KvmService.CHANNEL_ID,
            getString(R.string.notification_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = getString(R.string.notification_channel_description)
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java)
            .createNotificationChannel(channel)
    }
}