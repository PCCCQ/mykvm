package com.mykvm.receiver.input

import android.os.IBinder
import android.os.Parcel
import android.os.SystemClock
import android.view.InputDevice
import android.view.InputEvent
import android.view.KeyEvent
import android.view.MotionEvent
import rikka.shizuku.Shizuku
import rikka.shizuku.ShizukuBinderWrapper
import rikka.shizuku.SystemServiceHelper

/**
 * Injects input through `InputManager.injectInputEvent`, reached with the
 * `shell` user's INJECT_EVENTS permission via Shizuku.
 *
 * Why this route and not an AccessibilityService: `dispatchGesture()` can only
 * synthesise touch, there is no public API to inject a `KeyEvent` into another
 * app, and the KVM requirement is mouse **and keyboard**. Shizuku is the only
 * way to get that without root -- it is exactly what scrcpy relies on.
 *
 * The binder call is made by hand rather than through a generated AIDL stub:
 * `IInputManager` is a hidden interface, and `injectInputEvent` has been its
 * first declared method for every release we target, so its transaction code is
 * `FIRST_CALL_TRANSACTION`. Avoiding a checked-in copy of the AIDL also avoids
 * it silently drifting from the platform.
 */
class ShizukuInjector {

    @Volatile
    private var wrappedInputService: IBinder? = null

    /** Resolved once from the running framework; see [transactionCode]. */
    @Volatile
    private var cachedTransactionCode: Int? = null

    /** Set when injection is refused, so the service can surface it. */
    @Volatile
    var lastError: String? = null
        private set

    /** True when Shizuku is installed, running and this app is authorised. */
    fun isReady(): Boolean = try {
        Shizuku.pingBinder() &&
            Shizuku.checkSelfPermission() == android.content.pm.PackageManager.PERMISSION_GRANTED
    } catch (error: Throwable) {
        false
    }

    /** True when Shizuku is installed and its service is reachable at all. */
    fun isShizukuRunning(): Boolean = try {
        Shizuku.pingBinder()
    } catch (error: Throwable) {
        false
    }

    fun hasPermission(): Boolean = try {
        Shizuku.checkSelfPermission() == android.content.pm.PackageManager.PERMISSION_GRANTED
    } catch (error: Throwable) {
        false
    }

    /**
     * Drops the cached binder. Call this on Shizuku binder death or after a
     * permission change, otherwise every later event keeps transacting against
     * a dead proxy and silently fails.
     */
    fun invalidate() {
        wrappedInputService = null
    }

    /**
     * The AIDL transaction code for `injectInputEvent`.
     *
     * This cannot be a single hardcoded constant, and it cannot be read from the
     * running framework: the method's index shifts whenever AOSP inserts another
     * method ahead of it, and the generated `TRANSACTION_*` constants get inlined
     * by javac and stripped from the shipped framework (confirmed on device --
     * `NoSuchFieldException: TRANSACTION_injectInputEvent`).
     *
     * Values verified from AOSP
     * `core/java/android/hardware/input/IInputManager.aidl`:
     *
     *   API 28..32 (Android 9 .. 12L) -> 8
     *   API 33     (Android 13)       -> 9
     *   API 34     (Android 14)       -> 10
     *   API 35     (Android 15)       -> 11
     *
     * Getting this wrong fails *silently*: the service returns an empty reply,
     * which `readException()` reads as "no error", so the event is dropped
     * without a trace. The boolean-result check in `transactInject` is what turns
     * that into a visible failure.
     */
    private fun transactionCode(): Int {
        cachedTransactionCode?.let { return it }
        val sdk = android.os.Build.VERSION.SDK_INT
        val code = when {
            sdk >= 35 -> 11
            sdk == 34 -> 10
            sdk == 33 -> 9
            else -> 8
        }
        if (sdk > 35) {
            android.util.Log.w(TAG, "Android API $sdk is newer than the known table; guessing $code")
        }
        cachedTransactionCode = code
        android.util.Log.i(TAG, "injectInputEvent transaction code = $code (API $sdk)")
        return code
    }

    private fun inputService(): IBinder? {
        wrappedInputService?.let { if (it.isBinderAlive) return it }
        return try {
            val service = SystemServiceHelper.getSystemService("input") ?: return null
            ShizukuBinderWrapper(service).also { wrappedInputService = it }
        } catch (error: Throwable) {
            null
        }
    }

    private fun transactInject(event: InputEvent, mode: Int): Boolean {
        val service = inputService()
        if (service == null) {
            logThrottled("no input service binder (Shizuku not authorised?)")
            return false
        }
        val data = Parcel.obtain()
        val reply = Parcel.obtain()
        return try {
            data.writeInterfaceToken(INPUT_MANAGER_DESCRIPTOR)
            // AIDL writes a presence flag before an `in` Parcelable.
            data.writeInt(1)
            event.writeToParcel(data, 0)
            data.writeInt(mode)
            val code = transactionCode()
            service.transact(code, data, reply, 0)
            // Throws when the platform rejected the event, e.g. a malformed
            // key code, which is exactly the signal we want to surface.
            reply.readException()
            // injectInputEvent returns boolean; a wrong transaction code
            // yields an empty reply, which reads as 0 rather than throwing.
            if (reply.readInt() == 0) {
                lastError = "InputManager 拒绝了注入事件（Android 版本未适配？）"
                logThrottled("InputManager refused the event")
                return false
            }
            val count = injectedCount.incrementAndGet()
            if (count == 1 || count % 200 == 0) {
                android.util.Log.i(TAG, "injectInputEvent ok (#$count, ${event.javaClass.simpleName})")
            }
            true
        } catch (error: Throwable) {
            logThrottled("inject failed: ${error.javaClass.simpleName}: ${error.message}")
            false
        } finally {
            data.recycle()
            reply.recycle()
        }
    }

    /** A zero-duration press/release at (x, y). */
    fun tap(x: Float, y: Float, metaState: Int = 0): Boolean {
        val now = SystemClock.uptimeMillis()
        val down = touchEvent(MotionEvent.ACTION_DOWN, x, y, now, now, metaState) ?: return false
        val up = touchEvent(MotionEvent.ACTION_UP, x, y, now, now + 1, metaState) ?: return false
        return transactInject(down, MODE_ASYNC) and transactInject(up, MODE_ASYNC)
    }

    /** A press-and-hold long enough to open a context menu. */
    fun longPress(x: Float, y: Float, metaState: Int = 0): Boolean {
        val now = SystemClock.uptimeMillis()
        val down = touchEvent(MotionEvent.ACTION_DOWN, x, y, now, now, metaState) ?: return false
        val up = touchEvent(MotionEvent.ACTION_UP, x, y, now, now + LONG_PRESS_MS, metaState) ?: return false
        if (!transactInject(down, MODE_ASYNC)) return false
        SystemClock.sleep(LONG_PRESS_MS)
        return transactInject(up, MODE_ASYNC)
    }

    fun touchDown(x: Float, y: Float, downTime: Long, metaState: Int): Boolean {
        val event = touchEvent(MotionEvent.ACTION_DOWN, x, y, downTime, SystemClock.uptimeMillis(), metaState)
            ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    fun touchMove(x: Float, y: Float, downTime: Long, metaState: Int): Boolean {
        val event = touchEvent(MotionEvent.ACTION_MOVE, x, y, downTime, SystemClock.uptimeMillis(), metaState)
            ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    fun touchUp(x: Float, y: Float, downTime: Long, metaState: Int): Boolean {
        val event = touchEvent(MotionEvent.ACTION_UP, x, y, downTime, SystemClock.uptimeMillis(), metaState)
            ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    /**
     * Scrolls by synthesising a drag.
     *
     * Android's `ACTION_SCROLL` axis cannot be attached to a freshly obtained
     * `MotionEvent` (`setAxisValue` rejects an axis the event was not built
     * with), so a wheel notch is delivered the way every remote-control app
     * delivers it: as a short swipe that every app already understands.
     * [distance] is in pixels and must clear the system touch slop.
     */
    fun swipe(x: Float, y: Float, distance: Float, steps: Int = 4): Boolean {
        val now = SystemClock.uptimeMillis()
        val down = touchEvent(MotionEvent.ACTION_DOWN, x, y, now, now, 0) ?: return false
        if (!transactInject(down, MODE_ASYNC)) return false

        for (step in 1..steps) {
            val progress = step.toFloat() / steps
            val moveY = y + distance * progress
            val move = touchEvent(
                MotionEvent.ACTION_MOVE,
                x,
                moveY,
                now,
                now + (SWIPE_STEP_MS * step),
                0,
            )
            if (move != null) transactInject(move, MODE_ASYNC)
        }

        val up = touchEvent(MotionEvent.ACTION_UP, x, y + distance, now, now + SWIPE_STEP_MS * (steps + 1), 0)
        return up != null && transactInject(up, MODE_ASYNC)
    }

    fun key(keyCode: Int, down: Boolean, metaState: Int): Boolean {
        val eventTime = SystemClock.uptimeMillis()
        val event = try {
            KeyEvent(
                /* downTime = */ eventTime,
                /* eventTime = */ eventTime,
                /* action = */ if (down) KeyEvent.ACTION_DOWN else KeyEvent.ACTION_UP,
                /* code = */ keyCode,
                /* repeat = */ 0,
                /* metaState = */ metaState,
                /* deviceId = */ -1,
                /* scanCode = */ 0,
                /* flags = */ 0,
                /* source = */ InputDevice.SOURCE_KEYBOARD,
            )
        } catch (error: Throwable) {
            return false
        }
        return transactInject(event, MODE_ASYNC)
    }

    private fun touchEvent(
        action: Int,
        x: Float,
        y: Float,
        downTime: Long,
        eventTime: Long,
        metaState: Int,
    ): MotionEvent? = try {
        MotionEvent.obtain(downTime, eventTime, action, x, y, metaState).apply {
            source = InputDevice.SOURCE_TOUCHSCREEN
        }
    } catch (error: Throwable) {
        null
    }

    private val injectedCount = java.util.concurrent.atomic.AtomicInteger(0)

    private var lastLoggedAt = 0L

    /** Injection can fail per-event; do not let that flood logcat. */
    private fun logThrottled(message: String) {
        val now = SystemClock.uptimeMillis()
        if (now - lastLoggedAt < LOG_THROTTLE_MS) return
        lastLoggedAt = now
        android.util.Log.w(TAG, message)
    }

    companion object {
        private const val TAG = "MyKvmInjector"
        private const val INPUT_MANAGER_DESCRIPTOR = "android.hardware.input.IInputManager"

        /**
         * `injectInputEvent` is the first method declared in AOSP's
         * `IInputManager.aidl`, so AIDL assigns it FIRST_CALL_TRANSACTION.
         */
        private const val TRANSACTION_INJECT_INPUT_EVENT = IBinder.FIRST_CALL_TRANSACTION

        /** InputManager.INJECT_INPUT_EVENT_MODE_ASYNC */
        private const val MODE_ASYNC = 0

        private const val LONG_PRESS_MS = 600L
        private const val SWIPE_STEP_MS = 16L
        private const val LOG_THROTTLE_MS = 5_000L
    }
}
