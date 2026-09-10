package com.mykvm.receiver.input

import android.os.SystemClock
import android.view.KeyEvent
import org.json.JSONObject

/**
 * Turns wire events into injections.
 *
 * Wire contract (see the desktop's `input.rs`): `key_code` is a Windows virtual
 * key code, and coordinates are absolute device pixels because the screen is
 * advertised with `scale = 1.0`.
 *
 * Button mapping -- Android has no right-click, so:
 *   left    tap / press-drag-release
 *   right   long press (opens the context menu, the closest equivalent)
 *   middle  BACK
 *   side    BACK (HOME / Recents are blocked by AOSP, even via Shizuku)
 */
class InputDispatcher(
    private val injector: ShizukuInjector,
    private val cursor: VirtualCursor,
) {

    private var cursorX = 0f
    private var cursorY = 0f

    private var leftDown = false
    private var downTime = 0L

    private var metaState = 0

    /** True once the cursor has been positioned by the desktop. */
    var hasCursor = false
        private set

    fun handleEvent(event: JSONObject) {
        when (event.optString("type")) {
            "mouseMove" -> onMouseMove(event.optInt("x"), event.optInt("y"))
            "mouseButton" -> onMouseButton(event.optString("button"), event.optBoolean("down"))
            "scroll" -> onScroll(event.optInt("delta_x"), event.optInt("delta_y"))
            "key" -> onKey(event.optInt("key_code"), event.optBoolean("down"))
        }
    }

    private fun onMouseMove(x: Int, y: Int) {
        cursorX = x.toFloat()
        cursorY = y.toFloat()
        hasCursor = true
        cursor.moveTo(x, y)

        // Moving with the button held is a drag: forward it as a touch move so
        // the app under the cursor sees a continuous gesture.
        if (leftDown) {
            injector.touchMove(cursorX, cursorY, downTime, metaState)
        }
    }

    private fun onMouseButton(button: String, down: Boolean) {
        when (button) {
            "left" -> if (down) pressLeft() else releaseLeft()
            "right" -> if (down) injector.longPress(cursorX, cursorY, metaState)
            "middle" -> if (down) injectSystemKey(KeyEvent.KEYCODE_BACK)
            "back" -> if (down) injectSystemKey(KeyEvent.KEYCODE_BACK)
        // AOSP blocks HOME / APP_SWITCH injection even with INJECT_EVENTS,
        // so the forward side button repeats BACK rather than doing nothing.
        "forward" -> if (down) injectSystemKey(KeyEvent.KEYCODE_BACK)
        }
    }

    private fun pressLeft() {
        if (leftDown) return
        if (!injector.touchDown(cursorX, cursorY, SystemClock.uptimeMillis(), metaState)) return
        leftDown = true
        downTime = SystemClock.uptimeMillis()
    }

    private fun releaseLeft() {
        if (!leftDown) return
        injector.touchUp(cursorX, cursorY, downTime, metaState)
        leftDown = false
    }

    private fun onScroll(deltaX: Int, deltaY: Int) {
        // Positive delta_y means "scroll down" on the wire; content should move
        // up, so the finger drags upward.
        val distance = when {
            deltaY > 0 -> -SCROLL_STEP_PX * deltaY
            deltaY < 0 -> SCROLL_STEP_PX * -deltaY
            else -> 0f
        }
        if (distance != 0f) {
            injector.swipe(cursorX, cursorY, distance)
        }
        if (deltaX != 0) {
            injector.swipe(cursorX, cursorY, SCROLL_STEP_PX * -deltaX, steps = 3)
        }
    }

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
     * Releases anything still held. Called when the desktop disconnects: a lost
     * connection mid-drag would otherwise leave a touch pressed forever and the
     * phone unable to accept new input.
     */
    fun releaseAll() {
        if (leftDown) {
            injector.touchUp(cursorX, cursorY, downTime, 0)
            leftDown = false
        }
        // Release any modifier the desktop may still consider held.
        if (metaState != 0) {
            metaState = 0
        }
        cursor.hide()
    }

    private companion object {
        /**
         * Pixels of finger travel per wheel notch. Comfortably above the ~8-16px
         * touch slop so the gesture is read as a scroll rather than a tap.
         */
        const val SCROLL_STEP_PX = 80f
    }
}
