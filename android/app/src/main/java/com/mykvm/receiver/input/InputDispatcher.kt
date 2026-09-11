package com.mykvm.receiver.input

import android.os.SystemClock
import android.view.KeyEvent
import android.view.MotionEvent
import org.json.JSONObject

/**
 * Turns wire events into injections, in whichever [InputMode] the user picked.
 *
 * Wire contract (see the desktop's `input.rs`): `key_code` is a Windows virtual
 * key code, and coordinates are absolute device pixels because the screen is
 * advertised with `scale = 1.0`.
 *
 * Mouse mode maps buttons straight through:
 *   left / right / middle -> BUTTON_PRIMARY / SECONDARY / TERTIARY
 *   side buttons          -> BUTTON_BACK / BUTTON_FORWARD
 *
 * Touch mode keeps the fallbacks that Android forces on synthetic touches:
 *   right  -> long press,  middle/side -> BACK,  wheel -> short swipe
 */
class InputDispatcher(
    private val injector: ShizukuInjector,
    private val cursor: VirtualCursor,
    private val modeProvider: () -> InputMode,
) {

    private val mode: InputMode get() = modeProvider()

    private var cursorX = 0f
    private var cursorY = 0f

    /** Touch mode: the synthesised touch stream. */
    private var touchDown = false
    private var touchDownTime = 0L

    /** Mouse mode: which buttons are currently held, and when the drag began. */
    private var buttonState = 0
    private var mouseDownTime = 0L

    private var metaState = 0

    /** Last mode seen, so a switch can release whatever is held. */
    private var lastMode: InputMode = modeProvider()

    /** True once the cursor has been positioned by the desktop. */
    var hasCursor = false
        private set

    fun handleEvent(event: JSONObject) {
        // Switching modes mid-gesture must not strand a held button or touch in
        // the other mode.
        val current = mode
        if (current != lastMode) {
            releaseAll()
            lastMode = current
        }

        when (event.optString("type")) {
            "mouseMove" -> onMouseMove(event.optInt("x"), event.optInt("y"))
            "mouseButton" -> onMouseButton(event.optString("button"), event.optBoolean("down"))
            "scroll" -> onScroll(event.optInt("delta_x"), event.optInt("delta_y"))
            "key" -> onKey(event.optInt("key_code"), event.optBoolean("down"))
        }
    }

    /**
     * Paints the pointer ourselves, in both modes.
     *
     * Android does NOT render a cursor for injected `SOURCE_MOUSE` events: the
     * pointer controller only follows events from a real input device. Measured
     * on a Lenovo TB371FC (Android 14) -- `dumpsys input` keeps reporting
     * `PointerController: Presentation: SPOT` while mouse events are being
     * injected, and with this overlay disabled a full cursor sweep left nothing
     * visible on screen.
     *
     * So the overlay is required either way: touch mode needs it because a touch
     * device has no cursor at all, and mouse mode needs it because the platform
     * will not draw one for injected events.
     */
    private fun onMouseMove(x: Int, y: Int) {
        cursorX = x.toFloat()
        cursorY = y.toFloat()
        hasCursor = true
        cursor.moveTo(x, y)

        when (mode) {
            InputMode.MOUSE ->
                injector.mouseMove(cursorX, cursorY, buttonState, mouseDownTime, metaState)

            InputMode.TOUCH ->
                // Moving with the button held is a drag; otherwise nothing is
                // injected at all, because a touch device has no hover.
                if (touchDown) injector.touchMove(cursorX, cursorY, touchDownTime, metaState)
        }
    }

    private fun onMouseButton(button: String, down: Boolean) {
        when (mode) {
            InputMode.MOUSE -> onMouseButtonMouseMode(button, down)
            InputMode.TOUCH -> onMouseButtonTouchMode(button, down)
        }
    }

    // -----------------------------------------------------------------------
    // Mouse mode
    // -----------------------------------------------------------------------

    private fun onMouseButtonMouseMode(button: String, down: Boolean) {
        val mask = mouseButtonMask(button) ?: return
        val previous = buttonState

        if (down) {
            buttonState = previous or mask
            if (previous == 0) mouseDownTime = SystemClock.uptimeMillis()
            injector.mouseButton(true, cursorX, cursorY, buttonState, mouseDownTime, metaState)
            return
        }

        // A release we never saw the press for (e.g. the desktop reconnected
        // mid-drag) must not inject a stray button-up.
        if (previous and mask == 0) return
        injector.mouseButton(false, cursorX, cursorY, previous, mouseDownTime, metaState)
        buttonState = previous and mask.inv()
    }

    private fun mouseButtonMask(button: String): Int? = when (button) {
        "left" -> MotionEvent.BUTTON_PRIMARY
        "right" -> MotionEvent.BUTTON_SECONDARY
        "middle" -> MotionEvent.BUTTON_TERTIARY
        "back" -> MotionEvent.BUTTON_BACK
        "forward" -> MotionEvent.BUTTON_FORWARD
        else -> null
    }

    // -----------------------------------------------------------------------
    // Touch mode
    // -----------------------------------------------------------------------

    private fun onMouseButtonTouchMode(button: String, down: Boolean) {
        when (button) {
            "left" -> if (down) pressLeft() else releaseLeft()
            "right" -> if (down) injector.longPress(cursorX, cursorY, metaState)
            "middle", "back", "forward" -> if (down) injectSystemKey(KeyEvent.KEYCODE_BACK)
        }
    }

    private fun pressLeft() {
        if (touchDown) return
        if (!injector.touchDown(cursorX, cursorY, SystemClock.uptimeMillis(), metaState)) return
        touchDown = true
        touchDownTime = SystemClock.uptimeMillis()
    }

    private fun releaseLeft() {
        if (!touchDown) return
        injector.touchUp(cursorX, cursorY, touchDownTime, metaState)
        touchDown = false
    }

    // -----------------------------------------------------------------------
    // Scroll
    // -----------------------------------------------------------------------

    private fun onScroll(deltaX: Int, deltaY: Int) {
        when (mode) {
            InputMode.MOUSE -> {
                // A wheel reports how far it turned; positive vScroll scrolls
                // content up, which is a wheel rolled away from the user.
                if (deltaX == 0 && deltaY == 0) return
                injector.scroll(cursorX, cursorY, -deltaX.toFloat(), -deltaY.toFloat())
            }

            InputMode.TOUCH -> {
                // Positive delta_y means "scroll down" on the wire; content
                // should move up, so the finger drags upward.
                val distance = when {
                    deltaY > 0 -> -SCROLL_STEP_PX * deltaY
                    deltaY < 0 -> SCROLL_STEP_PX * -deltaY
                    else -> 0f
                }
                if (distance != 0f) injector.swipe(cursorX, cursorY, distance)
                if (deltaX != 0) injector.swipe(cursorX, cursorY, SCROLL_STEP_PX * -deltaX, steps = 3)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    private fun onKey(vk: Int, down: Boolean) {
        val keyCode = KeyMap.androidKeyCode(vk) ?: return

        // Modifiers have to update `metaState` *before* the event is built, so a
        // Shift press already carries META_SHIFT_ON and a Shift release does not.
        val bit = KeyMap.metaStateBit(vk)
        if (bit != 0) {
            metaState = if (down) metaState or bit else metaState and bit.inv()
        }

        injector.key(keyCode, down, metaState)
    }

    private fun injectSystemKey(keyCode: Int) {
        injector.key(keyCode, down = true, metaState = 0)
        injector.key(keyCode, down = false, metaState = 0)
    }

    /**
     * Releases anything still held. Called on mode switches and when the desktop
     * disconnects: a lost connection mid-drag would otherwise leave a button or
     * touch pressed forever and the tablet unable to accept new input.
     */
    fun releaseAll() {
        if (touchDown) {
            injector.touchUp(cursorX, cursorY, touchDownTime, 0)
            touchDown = false
        }
        if (buttonState != 0) {
            injector.mouseButton(
                down = false,
                x = cursorX,
                y = cursorY,
                buttonState = buttonState,
                downTime = mouseDownTime,
                metaState = 0,
            )
            buttonState = 0
        }
        metaState = 0
        cursor.hide()
    }

    private companion object {
        /**
         * Pixels of finger travel per wheel notch in touch mode. Comfortably
         * above the ~8-16px touch slop so the gesture reads as a scroll rather
         * than a tap.
         */
        const val SCROLL_STEP_PX = 80f
    }
}