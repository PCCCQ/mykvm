package com.mykvm.receiver

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Toast
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
import com.mykvm.receiver.diag.Diag
import com.mykvm.receiver.ui.ReceiverScreen
import org.json.JSONObject

class MainActivity : ComponentActivity() {

    private val notificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        NativeCore.ensureLoaded()
        Diag.init(this)
        val prefs = Prefs(this)
        ReceiverState.update {
            it.copy(
                keyboardPassthrough = prefs.keyboardPassthrough,
                imePermission = com.mykvm.receiver.input.ImeSuppressor.hasPermission(this),
                batteryExempt = KeepAlive.isExemptFromBatteryOptimizations(this),
                clipboardSync = prefs.clipboardSync,
            )
        }
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
                        onRestoreIme = {
                            // Safety net: puts a real IME back even if a previous
                            // run died mid-session.
                            com.mykvm.receiver.input.ImeSuppressor.restore(this, Prefs(this))
                        },
                        onClipboardSyncChange = { enabled ->
                            Prefs(this).clipboardSync = enabled
                            ReceiverState.update { it.copy(clipboardSync = enabled) }
                        },
                        onKeyboardPassthroughChange = { enabled ->
                            Prefs(this).keyboardPassthrough = enabled
                            ReceiverState.update { it.copy(keyboardPassthrough = enabled) }
                            // Apply immediately: restarting the service is the
                            // simplest way to re-run the suppress/restore pair.
                            if (ReceiverState.current.running) {
                                KvmService.stop(this)
                                KvmService.start(this)
                            }
                        },
                        onCursorSizeChange = { size ->
                            Prefs(this).cursorSizeDp = size
                            ReceiverState.update { it.copy(cursorSizeDp = size) }
                        },
                        onModeChange = { mode ->
                            // Persist for the next launch and publish immediately:
                            // the dispatcher reads the shared state per event, so
                            // the switch takes effect without restarting the service.
                            Prefs(this).inputMode = mode
                            ReceiverState.update { it.copy(inputMode = mode) }
                        },
                        onViewLog = {
                            val text = Diag.readText()
                            ReceiverState.update { it.copy(logText = text) }
                            if (text.isBlank()) toast("日志还是空的")
                        },
                        onClearLog = {
                            Diag.clear()
                            ReceiverState.update {
                                it.copy(logText = Diag.readText(), logNotice = "日志已清空")
                            }
                        },
                        onShareLog = ::shareLog,
                        onSendLog = ::sendLogToDesktop,
                        onRequestBatteryExemption = ::requestBatteryExemption,
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
        // Overlay, Shizuku and the battery-exemption state can all change while
        // we are backgrounded, so re-read them every time the screen comes back.
        ShizukuStatus.refresh(this)
        ReceiverState.update { it.copy(batteryExempt = KeepAlive.isExemptFromBatteryOptimizations(this)) }
        if (ReceiverState.current.running) {
            // Same entry point as every other restart path, so the foreground
            // service obligation is handled in exactly one place.
            KvmService.start(this)
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

    /** Re-reads the log so the in-app viewer shows the latest lines. */
    private fun viewLog() {
        val text = Diag.readText()
        ReceiverState.update { it.copy(logText = text) }
    }

    private fun shareLog() {
        val intent = Diag.shareIntent(this)
        if (intent == null) {
            toast("无法分享日志")
            return
        }
        try {
            startActivity(Intent.createChooser(intent, "分享 MyKVM 日志"))
        } catch (error: ActivityNotFoundException) {
            toast("没有可用的分享应用")
        }
    }

    /**
     * The one-tap path the user actually wants while testing: push the log to
     * the paired desktop, which stores it next to its own log file.
     */
    private fun sendLogToDesktop() {
        val text = Diag.readText()
        if (text.isBlank()) {
            ReceiverState.update { it.copy(logNotice = "日志还是空的") }
            toast("日志还是空的")
            return
        }

        val result = try {
            JSONObject(NativeCore.nativeSendDiagnostics(Diag.uploadName(), text))
        } catch (error: Throwable) {
            JSONObject().put("ok", false).put("error", error.message ?: "发送失败")
        }

        val notice = if (result.optBoolean("ok")) {
            "已发送到电脑（${result.optInt("bytes")} 字节），电脑端日志目录里可查看"
        } else {
            result.optString("error", "发送失败")
        }
        Diag.info("diagnostics upload: $notice")
        ReceiverState.update { it.copy(logNotice = notice) }
        toast(notice)
        viewLog()
    }

    /** Opens the system battery-optimisation exemption dialog. */
    private fun requestBatteryExemption() {
        if (KeepAlive.isExemptFromBatteryOptimizations(this)) {
            ReceiverState.update { it.copy(batteryExempt = true) }
            toast("已经在白名单里")
            return
        }
        val direct = Intent(
            Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
            Uri.parse("package:$packageName"),
        )
        try {
            startActivity(direct)
        } catch (error: ActivityNotFoundException) {
            // Some ROMs only expose the full list.
            try {
                startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
            } catch (fallback: Throwable) {
                toast("系统没有提供电池优化设置")
            }
        }
    }

    private fun toast(message: String) {
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
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
