package com.mykvm.receiver

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.core.content.ContextCompat
import com.mykvm.receiver.core.NativeCore
import com.mykvm.receiver.ui.ReceiverScreen

class MainActivity : ComponentActivity() {

    private val notificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        NativeCore.ensureLoaded()
        requestNotificationPermissionIfNeeded()

        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Surface(color = Color(0xFF101216), modifier = Modifier.fillMaxSize()) {
                    ReceiverScreen(
                        onToggleService = { enable ->
                            if (enable) KvmService.start(this) else KvmService.stop(this)
                        },
                        onGrantShizuku = ::requestShizukuPermission,
                        onOpenShizuku = ::openShizukuApp,
                        onGrantOverlay = ::requestOverlayPermission,
                        onUnpair = {
                            Prefs(this).unpair()
                            // Re-announce at once so the desktop drops us out of
                            // its layout instead of waiting for the peer TTL.
                            if (ReceiverState.current.running) {
                                KvmService.stop(this)
                                KvmService.start(this)
                            }
                        },
                    )
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        // Overlay and Shizuku permissions can change while we are backgrounded.
        ShizukuStatus.refresh(this)
        if (ReceiverState.current.running) {
            startService(Intent(this, KvmService::class.java))
        }
    }

    private fun requestNotificationPermissionIfNeeded() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        val granted = ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED
        if (!granted) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    private fun requestShizukuPermission() {
        if (!ShizukuStatus.requestPermission(SHIZUKU_REQUEST_CODE)) {
            openShizukuApp()
        }
    }

    private fun openShizukuApp() {
        val intent = packageManager.getLaunchIntentForPackage(SHIZUKU_PACKAGE)
        if (intent != null) {
            startActivity(intent)
        } else {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://shizuku.rikka.app/")))
        }
    }

    private fun requestOverlayPermission() {
        startActivity(
            Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                Uri.parse("package:$packageName"),
            ),
        )
    }

    private companion object {
        const val SHIZUKU_REQUEST_CODE = 4001
        const val SHIZUKU_PACKAGE = "moe.shizuku.privileged.api"
    }
}
