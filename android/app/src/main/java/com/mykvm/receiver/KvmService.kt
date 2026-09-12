package com.mykvm.receiver

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.ClipData
import android.content.ClipboardManager
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import com.mykvm.receiver.core.NativeCore
import com.mykvm.receiver.input.InputDispatcher
import com.mykvm.receiver.input.ShizukuInjector
import com.mykvm.receiver.input.VirtualCursor
import com.mykvm.receiver.input.InputMode
import com.mykvm.receiver.input.ImeSuppressor
import com.mykvm.receiver.diag.Diag
import org.json.JSONObject
import rikka.shizuku.Shizuku
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * Owns the receiver: the Rust core, the injection pipeline and the wake locks.
 *
 * It must be a foreground service -- Android suspends an ordinary background
 * process within minutes, which would drop the QUIC connection exactly when the
 * user is not looking at the phone (i.e. the entire point of the app).
 */
class KvmService : Service() {

    private val mainHandler = Handler(Looper.getMainLooper())

    private lateinit var prefs: Prefs
    private lateinit var injector: ShizukuInjector
    private lateinit var cursor: VirtualCursor
    private lateinit var dispatcher: InputDispatcher

    private var pollThread: Thread? = null
    private val running = AtomicBoolean(false)

    /**
     * Watches the poll thread. A foreground service survives most things, but
     * an aggressive ROM can still freeze or kill the process; if the poll
     * thread is gone while we believe we are running, restart in place instead
     * of sitting there looking healthy with a dead core.
     */
    private val watchdog = object : Runnable {
        override fun run() {
            if (!running.get()) return
            val thread = pollThread
            if (thread == null || !thread.isAlive) {
                Diag.warn("poll thread is gone; restarting the receiver")
                stopReceiver()
                startReceiver()
                return
            }
            // No need to touch AlarmManager here: it re-arms itself in
            // KeepAliveReceiver, so a binder call every 15s would be waste.
            mainHandler.postDelayed(this, WATCHDOG_INTERVAL_MS)
        }
    }

    /** Last notification text, so a keep-alive restart does not blank it. */
    private var lastNotificationText: String? = null

    private var multicastLock: WifiManager.MulticastLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var wakeLock: PowerManager.WakeLock? = null

    private val shizukuPermissionListener =
        Shizuku.OnRequestPermissionResultListener { requestCode, grantResult ->
            if (requestCode == SHIZUKU_PERMISSION_REQUEST) {
                injector.invalidate()
                publishShizukuState()
                if (grantResult == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                    Log.i(TAG, "Shizuku permission granted")
                } else {
                    ReceiverState.update { it.copy(lastError = "Shizuku 授权被拒绝") }
                }
            }
        }

    private val shizukuBinderListener = Shizuku.OnBinderReceivedListener {
        // A restarted Shizuku gets a new binder; the cached wrapper is stale.
        injector.invalidate()
        publishShizukuState()
    }

    private val shizukuDeadListener = Shizuku.OnBinderDeadListener {
        injector.invalidate()
        publishShizukuState()
    }

    override fun onCreate() {
        super.onCreate()
        // First, so the very first thing that can go wrong is already recorded.
        Diag.init(this)
        prefs = Prefs(this)
        injector = ShizukuInjector()
        // Read the size on the main thread each time the overlay is rebuilt, so
        // the slider in the UI takes effect without restarting the service.
        cursor = VirtualCursor(this) { dpToPx(prefs.cursorSizeDp) }
        // The mode is read per event from the shared state, so flipping the
        // switch in the UI takes effect immediately without a restart.
        dispatcher = InputDispatcher(injector, cursor) { ReceiverState.current.inputMode }

        try {
            Shizuku.addRequestPermissionResultListener(shizukuPermissionListener)
            Shizuku.addBinderReceivedListenerSticky(shizukuBinderListener)
            Shizuku.addBinderDeadListener(shizukuDeadListener)
        } catch (error: Throwable) {
            Log.w(TAG, "Shizuku listeners unavailable: ${error.message}")
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                prefs.shouldRun = false
                KeepAlive.cancel(this)
                stopReceiver()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> {
                prefs.shouldRun = true
                startReceiver()
            }
        }
        return START_STICKY
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        // Screen size feeds the coordinates the desktop maps its cursor into, so
        // a rotation invalidates them -- but only the *size* changed. Restarting
        // the receiver for that used to tear down the QUIC endpoint, which
        // dropped the desktop's live connection and made it log "the server
        // refused to accept a new connection" until the new endpoint came up.
        // Re-advertising the size in place keeps the connection alive.
        Log.i(TAG, "display configuration changed; re-advertising screen size")
        Diag.info("display configuration changed: $newConfig")
        publishScreenSize()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        stopReceiver()
        try {
            Shizuku.removeRequestPermissionResultListener(shizukuPermissionListener)
            Shizuku.removeBinderReceivedListener(shizukuBinderListener)
            Shizuku.removeBinderDeadListener(shizukuDeadListener)
        } catch (error: Throwable) {
            // Shizuku may have gone away; nothing to clean up then.
        }
        super.onDestroy()
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    private fun startReceiver() {
        // Post the notification *before* the "are we already running" check: a
        // restart from the alarm or the boot receiver arrives through
        // startForegroundService, which obliges us to be foreground within a few
        // seconds even when the receiver never stopped.
        startForegroundNotification(
            lastNotificationText ?: getString(R.string.notification_idle),
        )
        if (running.getAndSet(true)) return

        acquireLocks()

        // Take the tablet's IME out of the way so injected keys reach the app
        // instead of being composed. Restored in stopReceiver().
        if (prefs.keyboardPassthrough) {
            ImeSuppressor.suppress(this, prefs)
        }

        if (!NativeCore.ensureLoaded()) {
            running.set(false)
            ReceiverState.update {
                it.copy(running = false, lastError = "未能加载本地库（ABI 不支持？）")
            }
            return
        }

        val bounds = currentDisplayBounds()
        Diag.info("starting receiver: screen=${bounds.first}x${bounds.second}")
        val config = JSONObject().apply {
            put("deviceName", prefs.deviceName)
            put("stableId", prefs.stableId)
            put("appVersion", BuildConfig.VERSION_NAME)
            put("screenId", SCREEN_ID)
            put("screenWidth", bounds.first)
            put("screenHeight", bounds.second)
            put("discoveryPort", DISCOVERY_PORT)
            put("quicPort", QUIC_PORT)
            put("identityDir", filesDir.absolutePath)
            // The Rust core tees its own log records into this same file, so one
            // upload carries both the app and the protocol side.
            put("logFile", Diag.path(this@KvmService))
            // The persisted layout is Rust's own JSON; pass it back verbatim.
            prefs.layoutJson?.let { put("layout", JSONObject(it)) }
        }

        val response = try {
            JSONObject(NativeCore.nativeStart(config.toString()))
        } catch (error: Throwable) {
            JSONObject().put("ok", false).put("error", error.message ?: "native start failed")
        }

        if (!response.optBoolean("ok")) {
            running.set(false)
            val message = response.optString("error", "启动失败")
            Diag.error("native start failed: $message")
            ReceiverState.update { it.copy(running = false, lastError = message) }
            stopForeground(STOP_FOREGROUND_REMOVE)
            return
        }

        ReceiverState.update {
            it.copy(
                running = true,
                inputMode = prefs.inputMode,
                paired = response.optBoolean("paired", false),
                deviceName = prefs.deviceName,
                peerId = response.optString("peerId").takeIf { value -> value.isNotBlank() },
                quicPort = response.optInt("quicPort"),
                discoveryPort = response.optInt("discoveryPort"),
                lastError = null,
                overlayGranted = Settings.canDrawOverlays(this),
            )
        }
        publishShizukuState()

        pollThread = thread(name = "mykvm-poll", isDaemon = true) { pollLoop() }
        // The process may be killed while the tablet sleeps, so leave an alarm
        // behind that can bring the service back without the user noticing.
        KeepAlive.schedule(this)
        mainHandler.removeCallbacks(watchdog)
        mainHandler.postDelayed(watchdog, WATCHDOG_INTERVAL_MS)
    }

    private fun stopReceiver() {
        if (!running.getAndSet(false)) return
        Diag.info("stopping receiver")
        mainHandler.removeCallbacks(watchdog)
        pollThread?.join(500)
        pollThread = null
        dispatcher.releaseAll()
        try {
            NativeCore.nativeStop()
        } catch (error: Throwable) {
            Log.w(TAG, "nativeStop failed: ${error.message}")
        }
        ImeSuppressor.restore(this, prefs)
        releaseLocks()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        ReceiverState.update {
            it.copy(running = false, pairingCode = null, connectedController = null)
        }
    }

    /**
     * Re-advertises the current display size without restarting the receiver.
     *
     * The discovery loop reads the size out of the shared screen on every
     * announce (3s), so the desktop picks the new geometry up on its own -- no
     * reconnect, no dropped input.
     */
    private fun publishScreenSize() {
        if (!running.get()) return
        val bounds = currentDisplayBounds()
        try {
            val response = JSONObject(NativeCore.nativeSetScreenSize(bounds.first, bounds.second))
            if (!response.optBoolean("ok")) {
                Diag.warn("nativeSetScreenSize failed: ${response.optString("error")}")
            }
        } catch (error: Throwable) {
            Diag.warn("nativeSetScreenSize threw", error)
        }
    }

    private fun pollLoop() {
        var lastBoundsCheck = SystemClock.elapsedRealtime()
        var lastBounds = currentDisplayBounds()
        var lastCursorSizeDp = prefs.cursorSizeDp

        while (running.get()) {
            // PC mode lets the window be dragged and resized, and the display
            // itself can change; both invalidate the geometry the desktop maps
            // its cursor into, so re-advertise when it moves.
            val now = SystemClock.elapsedRealtime()
            val cursorSizeDp = prefs.cursorSizeDp
            if (cursorSizeDp != lastCursorSizeDp) {
                lastCursorSizeDp = cursorSizeDp
                cursor.refreshAppearance()
            }
            if (now - lastBoundsCheck >= BOUNDS_CHECK_INTERVAL_MS) {
                lastBoundsCheck = now
                val bounds = currentDisplayBounds()
                if (bounds != lastBounds) {
                    Log.i(TAG, "display bounds changed: $lastBounds -> $bounds")
                    Diag.info("display bounds changed: $lastBounds -> $bounds")
                    lastBounds = bounds
                    publishScreenSize()
                }
            }
            val payload = try {
                NativeCore.nativePoll(POLL_TIMEOUT_MS)
            } catch (error: Throwable) {
                Log.e(TAG, "poll failed: ${error.message}")
                Diag.error("native poll failed; receiver stopped", error)
                break
            } ?: continue

            try {
                handleEvent(JSONObject(payload))
            } catch (error: Throwable) {
                Log.w(TAG, "bad event payload: ${error.message}")
            }
        }
    }

    private fun handleEvent(event: JSONObject) {
        when (event.optString("type")) {
            "input" -> {
                val input = event.optJSONObject("event") ?: return
                // moveTo() shows the overlay itself; no need to pre-show.
                dispatcher.handleEvent(input)
            }

            "pairingRequested" -> {
                Diag.info(
                    "pairing requested by ${event.optString("requesterName")} " +
                        "(${event.optString("requesterIp")})",
                )
                ReceiverState.update {
                    it.copy(
                        pairingCode = event.optString("code"),
                        pairingRequester = event.optString("requesterName"),
                        lastError = null,
                    )
                }
            }

            "pairingCleared" -> ReceiverState.update { it.copy(pairingCode = null) }

            "clipboardText" -> applyClipboardText(event.optString("text"))

            "pairingFailed" -> {
                Diag.warn("pairing failed: ${event.optString("reason")}")
                ReceiverState.update {
                    it.copy(lastError = event.optString("reason"))
                }
            }

            "paired" -> {
                // Persist exactly what Rust produced so the next launch restores
                // a working pairing without a round trip.
                event.optJSONObject("layout")?.let { prefs.layoutJson = it.toString() }
                val controller = event.optString("controllerName")
                    .takeIf { it.isNotBlank() }
                    ?: event.optString("controllerId")
                Diag.info("paired with $controller")
                ReceiverState.update {
                    it.copy(
                        paired = true,
                        pairingCode = null,
                        connectedController = controller,
                        lastError = null,
                    )
                }
                updateNotification(getString(R.string.notification_paired, controller))
            }

            "peerPresence" -> {
                val ids = event.optJSONArray("peerIds")
                val first = if (ids != null && ids.length() > 0) ids.optString(0) else null
                ReceiverState.update { it.copy(connectedController = first ?: it.connectedController) }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Shizuku / permissions
    // -----------------------------------------------------------------------

    fun publishShizukuState() {
        // The injector caches its binder; drop it first so the probe below
        // reflects a Shizuku that restarted in the meantime.
        injector.invalidate()
        ShizukuStatus.refresh(this)
    }

    // -----------------------------------------------------------------------
    // Locks, notification, display
    // -----------------------------------------------------------------------

    private fun acquireLocks() {
        try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            // Broadcast UDP is dropped while the Wi-Fi radio naps; this is the
            // difference between discovering the desktop and never seeing it.
            multicastLock = wifi.createMulticastLock("mykvm-discovery").apply {
                setReferenceCounted(false)
                acquire()
            }
            // LOW_LATENCY (API 29+) keeps the radio out of power save without
            // the throughput-biased behaviour of the deprecated HIGH_PERF mode,
            // which matters for a stream of small input datagrams.
            val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
                @Suppress("DEPRECATION")
                WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            wifiLock = wifi.createWifiLock(mode, "mykvm-wifi").apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (error: Throwable) {
            Log.w(TAG, "wifi locks unavailable: ${error.message}")
        }

        try {
            val power = getSystemService(Context.POWER_SERVICE) as PowerManager
            wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "mykvm:receiver").apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (error: Throwable) {
            Log.w(TAG, "wake lock unavailable: ${error.message}")
        }
    }

    private fun releaseLocks() {
        listOf(multicastLock, wifiLock, wakeLock).forEach { lock ->
            try {
                when (lock) {
                    null -> Unit
                    is WifiManager.MulticastLock -> if (lock.isHeld) lock.release()
                    is WifiManager.WifiLock -> if (lock.isHeld) lock.release()
                    is PowerManager.WakeLock -> if (lock.isHeld) lock.release()
                }
            } catch (error: Throwable) {
                Log.w(TAG, "releasing a lock failed: ${error.message}")
            }
        }
        multicastLock = null
        wifiLock = null
        wakeLock = null
    }

    private fun startForegroundNotification(text: String) {
        lastNotificationText = text
        val notification = buildNotification(text)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun updateNotification(text: String) {
        lastNotificationText = text
        val manager = androidx.core.app.NotificationManagerCompat.from(this)
        if (manager.areNotificationsEnabled()) {
            manager.notify(NOTIFICATION_ID, buildNotification(text))
        }
    }

    private fun buildNotification(text: String): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, KvmService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.notification_title))
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_launcher)
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(0, "停止", stop)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    /** Full display size in pixels -- the coordinate space touches use. */
    /**
     * Full display size in pixels -- the coordinate space touches use.
     *
     * Deliberately NOT `currentWindowMetrics`: in the tablet's PC mode the
     * receiver runs inside a freeform window, so currentWindowMetrics would
     * report the window (e.g. 1488x1097) and the desktop would map its cursor
     * into a fraction of the screen. `maximumWindowMetrics` reports the largest
     * area the app could occupy, which is the display itself -- verified on
     * device with mMaxBounds=Rect(0,0-2944,1840) while the window sat at
     * Rect(558,483-2046,1580).
     */
    /** dp -> physical pixels, using the display the app is currently on. */
    private fun dpToPx(dp: Int): Int {
        val metrics = resources.displayMetrics
        return (dp * metrics.density).toInt().coerceAtLeast(1)
    }

    /**
     * Mirrors clipboard text from the desktop. Only this direction: Android
     * 10+ blocks a background app from reading the clipboard, so sending the
     * device clipboard back would need an accessibility service.
     */
    private fun applyClipboardText(text: String) {
        if (!prefs.clipboardSync || text.isEmpty()) return
        try {
            val clipboard = getSystemService(ClipboardManager::class.java)
            clipboard.setPrimaryClip(ClipData.newPlainText("MyKVM", text))
            Log.i(TAG, "clipboard updated (${text.length} chars)")
        } catch (error: Throwable) {
            Log.w(TAG, "cannot write clipboard: ${error.message}")
        }
    }

    private fun currentDisplayBounds(): Pair<Int, Int> {
        return try {
            val windowManager = getSystemService(WindowManager::class.java)
            val bounds = windowManager.maximumWindowMetrics.bounds
            bounds.width() to bounds.height()
        } catch (error: Throwable) {
            val metrics = resources.displayMetrics
            metrics.widthPixels to metrics.heightPixels
        }
    }

    companion object {
        private const val TAG = "MyKvmService"
        const val CHANNEL_ID = "mykvm-receiver"
        private const val NOTIFICATION_ID = 47834
        private const val SHIZUKU_PERMISSION_REQUEST = 4001
        private const val POLL_TIMEOUT_MS = 200L

        /** How often the service checks that its poll thread is still alive. */
        private const val WATCHDOG_INTERVAL_MS = 15_000L

        /** Must match the desktop's discovery/QUIC defaults. */
        private const val DISCOVERY_PORT = 47833
        private const val QUIC_PORT = 47834

        private const val SCREEN_ID = "android-screen-1"

        /** How often the display geometry is re-checked while running. */
        private const val BOUNDS_CHECK_INTERVAL_MS = 3000L

        const val ACTION_STOP = "com.mykvm.receiver.STOP"

        fun start(context: Context) {
            val intent = Intent(context, KvmService::class.java)
            context.startForegroundService(intent)
        }

        /**
         * Stops the receiver for good.
         *
         * The intent can be refused (Android 12+ blocks a background
         * `startService`), so the "do not come back" flag is written first --
         * a refused delivery must not leave the service to be resurrected by
         * the keep-alive alarm.
         */
        fun stop(context: Context) {
            Prefs(context).shouldRun = false
            KeepAlive.cancel(context)
            val intent = Intent(context, KvmService::class.java).setAction(ACTION_STOP)
            try {
                context.startService(intent)
            } catch (error: Throwable) {
                Diag.warn("stop intent refused; stopping the service directly", error)
                context.stopService(Intent(context, KvmService::class.java))
            }
        }

        fun requestShizukuPermission(requestCode: Int = SHIZUKU_PERMISSION_REQUEST) {
            Shizuku.requestPermission(requestCode)
        }
    }
}
