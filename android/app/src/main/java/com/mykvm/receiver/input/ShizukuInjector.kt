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
 * Two event families live here:
 *
 *  * **touch** (`SOURCE_TOUCHSCREEN`) -- synthesised taps and swipes. Works in
 *    every app, but cannot express hover, a real right-click or a real wheel.
 *  * **mouse** (`SOURCE_MOUSE`) -- hover moves, per-button press/release and
 *    `ACTION_SCROLL`. Android's own mouse stack handles these, so mouse-aware
 *    apps (browsers, office, remote desktop, emulators) behave natively.
 *
 * [InputDispatcher] picks between them at runtime.
 *
 * The binder call is made by hand rather than through a generated AIDL stub:
 * `IInputManager` is hidden, and its transaction codes shift between releases
 * (see [transactionCode]).
 */
class ShizukuInjector {

    @Volatile
    private var wrappedInputService: IBinder? = null

    /** Resolved once per process; see [transactionCode]. */
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

    // -----------------------------------------------------------------------
    // Mouse events (SOURCE_MOUSE)
    // -----------------------------------------------------------------------

    /**
     * Reports the pointer position the way a real mouse does.
     *
     * With no button held a real mouse sends `ACTION_HOVER_MOVE`, which is what
     * drives hover highlights; while dragging it sends `ACTION_MOVE` carrying
     * the held buttons. Getting this distinction right is the difference
     * between apps seeing a mouse and seeing a stuck drag.
     */
    fun mouseMove(x: Float, y: Float, buttonState: Int, downTime: Long, metaState: Int): Boolean {
        val now = SystemClock.uptimeMillis()
        val action = if (buttonState == 0) MotionEvent.ACTION_HOVER_MOVE else MotionEvent.ACTION_MOVE
        val event = motionEvent(
            action = action,
            x = x,
            y = y,
            downTime = if (buttonState == 0) now else downTime,
            eventTime = now,
            metaState = metaState,
            buttonState = buttonState,
            source = InputDevice.SOURCE_MOUSE,
            toolType = MotionEvent.TOOL_TYPE_MOUSE,
        ) ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    /**
     * Press or release of a single mouse button.
     *
     * [buttonState] is the state *after* the transition for a press and *before*
     * it for a release -- exactly how the platform reports physical mice.
     */
    fun mouseButton(
        down: Boolean,
        x: Float,
        y: Float,
        buttonState: Int,
        downTime: Long,
        metaState: Int,
    ): Boolean {
        val now = SystemClock.uptimeMillis()
        val event = motionEvent(
            action = if (down) MotionEvent.ACTION_DOWN else MotionEvent.ACTION_UP,
            x = x,
            y = y,
            downTime = if (down) now else downTime,
            eventTime = now,
            metaState = metaState,
            buttonState = buttonState,
            source = InputDevice.SOURCE_MOUSE,
            toolType = MotionEvent.TOOL_TYPE_MOUSE,
        ) ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    /** `ACTION_HOVER_ENTER` / `ACTION_HOVER_EXIT`, so apps see the pointer leave. */
    fun hoverBoundary(enter: Boolean, x: Float, y: Float): Boolean {
        val now = SystemClock.uptimeMillis()
        val event = motionEvent(
            action = if (enter) MotionEvent.ACTION_HOVER_ENTER else MotionEvent.ACTION_HOVER_EXIT,
            x = x,
            y = y,
            downTime = now,
            eventTime = now,
            source = InputDevice.SOURCE_MOUSE,
            toolType = MotionEvent.TOOL_TYPE_MOUSE,
        ) ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    /**
     * A real scroll wheel notch.
     *
     * `PointerCoords.setAxisValue` is public, so the scroll axes can be built
     * into the event directly -- no `ACTION_SCROLL` axis workaround and no
     * synthetic swipe. Positive [vScroll] scrolls content up, matching a wheel
     * rolled away from the user.
     */
    fun scroll(x: Float, y: Float, hScroll: Float, vScroll: Float): Boolean {
        val now = SystemClock.uptimeMillis()
        val event = motionEvent(
            action = MotionEvent.ACTION_SCROLL,
            x = x,
            y = y,
            downTime = now,
            eventTime = now,
            source = InputDevice.SOURCE_MOUSE,
            toolType = MotionEvent.TOOL_TYPE_MOUSE,
            hScroll = hScroll,
            vScroll = vScroll,
        ) ?: return false
        return transactInject(event, MODE_ASYNC)
    }

    // -----------------------------------------------------------------------
    // Touch events (SOURCE_TOUCHSCREEN)
    // -----------------------------------------------------------------------

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
     * Scrolls a touch-only app by synthesising a drag.
     *
     * Used by touch mode: [ACTION_SCROLL] is a mouse concept and plain views
     * ignore it, so a wheel notch has to arrive as the swipe every app already
     * understands. [distance] is in pixels and must clear the touch slop.
     */
    fun swipe(x: Float, y: Float, distance: Float, steps: Int = 4): Boolean {
        val now = SystemClock.uptimeMillis()
        val down = touchEvent(MotionEvent.ACTION_DOWN, x, y, now, now, 0) ?: return false
        if (!transactInject(down, MODE_ASYNC)) return false

        for (step in 1..steps) {
            val progress = step.toFloat() / steps
            val move = touchEvent(
                MotionEvent.ACTION_MOVE,
                x,
                y + distance * progress,
                now,
                now + (SWIPE_STEP_MS * step),
                0,
            )
            if (move != null) transactInject(move, MODE_ASYNC)
        }

        val up = touchEvent(MotionEvent.ACTION_UP, x, y + distance, now, now + SWIPE_STEP_MS * (steps + 1), 0)
        return up != null && transactInject(up, MODE_ASYNC)
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Event construction
    // -----------------------------------------------------------------------

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

    /**
     * Builds a fully specified event.
     *
     * The six argument `MotionEvent.obtain` cannot express a button state or a
     * scroll axis, and neither `setButtonState` nor `setAxisValue` exists on
     * `MotionEvent` itself. The public overload that takes `PointerProperties`
     * and `PointerCoords` can express all of it, and `PointerCoords` does have
     * a public `setAxisValue` -- which is what makes a real wheel possible.
     */
    private fun motionEvent(
        action: Int,
        x: Float,
        y: Float,
        downTime: Long,
        eventTime: Long,
        metaState: Int = 0,
        buttonState: Int = 0,
        source: Int = InputDevice.SOURCE_TOUCHSCREEN,
        toolType: Int = MotionEvent.TOOL_TYPE_FINGER,
        hScroll: Float = 0f,
        vScroll: Float = 0f,
    ): MotionEvent? = try {
        val properties = arrayOf(
            MotionEvent.PointerProperties().apply {
                id = 0
                this.toolType = toolType
            },
        )
        val coords = arrayOf(
            MotionEvent.PointerCoords().apply {
                this.x = x
                this.y = y
                pressure = 1f
                size = 1f
                if (hScroll != 0f) setAxisValue(MotionEvent.AXIS_HSCROLL, hScroll)
                if (vScroll != 0f) setAxisValue(MotionEvent.AXIS_VSCROLL, vScroll)
            },
        )
        MotionEvent.obtain(
            downTime,
            eventTime,
            action,
            /* pointerCount = */ 1,
            properties,
            coords,
            metaState,
            buttonState,
            /* xPrecision = */ 1f,
            /* yPrecision = */ 1f,
            /* deviceId = */ 0,
            /* edgeFlags = */ 0,
            source,
            /* flags = */ 0,
        )
    } catch (error: Throwable) {
        logThrottled("cannot build motion event: ${error.javaClass.simpleName}: ${error.message}")
        null
    }

    // -----------------------------------------------------------------------
    // Binder plumbing
    // -----------------------------------------------------------------------

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
     * without a trace. The boolean-result check in [transactInject] is what turns
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

        /** InputManager.INJECT_INPUT_EVENT_MODE_ASYNC */
        private const val MODE_ASYNC = 0

        private const val LONG_PRESS_MS = 600L
        private const val SWIPE_STEP_MS = 16L
        private const val LOG_THROTTLE_MS = 5_000L
    }
}