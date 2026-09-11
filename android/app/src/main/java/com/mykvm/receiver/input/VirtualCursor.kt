package com.mykvm.receiver.input

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.WindowManager

/**
 * The pointer the user sees on the phone.
 *
 * Android has no system cursor for a touch device, and it does not render one
 * for injected `SOURCE_MOUSE` events either (the pointer controller only follows
 * real input devices), so the receiver paints its own: a small always-on-top
 * window whose tip tracks the coordinates the desktop sends. Mouse movement
 * never reaches the injected input stream -- only clicks, drags and scrolls do,
 * which is what keeps hovering free of jank.
 *
 * Every WindowManager call and every View construction happens on the main
 * thread. `WindowManager.addView` builds a `ViewRootImpl`, which installs a
 * `Handler` on the calling thread, so calling it from the poll thread fails with
 * "Can't create handler inside thread that has not called Looper.prepare()".
 * Positions are coalesced: a burst of mouse moves costs one relayout per
 * main-thread pass rather than one per event.
 *
 * [sizePxProvider] is read on the main thread so the user can resize the cursor
 * from the UI while the service keeps running.
 */
class VirtualCursor(
    private val context: Context,
    private val sizePxProvider: () -> Int,
) {

    private val handler = Handler(Looper.getMainLooper())
    private val windowManager = context.getSystemService(WindowManager::class.java)

    /** Only ever touched on the main thread. */
    private var view: CursorView? = null
    private var layoutParams: WindowManager.LayoutParams? = null
    private var appliedSizePx = 0

    /** Set from the poll thread, consumed on the main thread. */
    @Volatile
    private var visible = false

    private var pendingX = 0
    private var pendingY = 0
    private var applyScheduled = false

    val isVisible: Boolean get() = visible && view != null

    private val apply = Runnable {
        applyScheduled = false
        applyOnMainThread()
    }

    /** Moves the cursor; also makes it visible if it was hidden. */
    fun moveTo(x: Int, y: Int) {
        synchronized(this) {
            pendingX = x
            pendingY = y
            visible = true
        }
        scheduleApply()
    }

    fun show() {
        visible = true
        scheduleApply()
    }

    fun hide() {
        visible = false
        scheduleApply()
    }

    /** Re-applies the current size, e.g. after the user drags the slider. */
    fun refreshAppearance() {
        scheduleApply()
    }

    private fun scheduleApply() {
        if (applyScheduled) return
        applyScheduled = true
        handler.post(apply)
    }

    private fun applyOnMainThread() {
        if (!visible) {
            detach()
            return
        }

        val sizePx = sizePxProvider().coerceIn(MIN_SIZE_PX, MAX_SIZE_PX)
        // A size change cannot be applied to an existing window, so rebuild it.
        if (view != null && sizePx != appliedSizePx) {
            detach()
        }

        val x = pendingX
        val y = pendingY
        val existing = view

        if (existing == null) {
            // Constructed here, not in the constructor, so the View (which
            // touches a Handler internally) is always built on the main thread.
            val created = CursorView(context, sizePx)
            val params = WindowManager.LayoutParams(
                sizePx,
                sizePx,
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                    WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                    WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                    WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
                    WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED,
                PixelFormat.TRANSLUCENT,
            ).apply {
                gravity = Gravity.TOP or Gravity.START
                this.x = x
                this.y = y
            }

            try {
                windowManager.addView(created, params)
                view = created
                layoutParams = params
                appliedSizePx = sizePx
            } catch (error: Throwable) {
                Log.w(TAG, "cannot show cursor overlay: ${error.message}")
                view = null
                layoutParams = null
                appliedSizePx = 0
            }
            return
        }

        val params = layoutParams ?: return
        if (params.x == x && params.y == y) return
        params.x = x
        params.y = y
        try {
            windowManager.updateViewLayout(existing, params)
        } catch (error: Throwable) {
            // The window was torn down underneath us; drop it so the next move
            // re-adds it.
            Log.w(TAG, "cursor overlay lost: ${error.message}")
            view = null
            layoutParams = null
            appliedSizePx = 0
        }
    }

    private fun detach() {
        view?.let { current ->
            try {
                windowManager.removeView(current)
            } catch (error: Throwable) {
                // Already detached.
            }
        }
        view = null
        layoutParams = null
        appliedSizePx = 0
    }

    /**
     * Draws an arrow whose tip sits at the window's top-left corner.
     *
     * The path is authored in a 40x40 box and scaled to the requested size, so
     * the outline stays proportional at any setting.
     */
    private class CursorView(context: Context, sizePx: Int) : View(context) {

        private val scale = sizePx / DESIGN_BOX

        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.WHITE
            style = Paint.Style.FILL
        }
        private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.argb(220, 0, 0, 0)
            style = Paint.Style.STROKE
            strokeWidth = (sizePx / 16f).coerceIn(2f, 6f)
            strokeJoin = Paint.Join.ROUND
        }
        private val arrow = Path().apply {
            moveTo(1f, 1f)
            lineTo(1f, 34f)
            lineTo(10f, 26f)
            lineTo(17f, 40f)
            lineTo(23f, 37f)
            lineTo(16f, 24f)
            lineTo(27f, 22f)
            close()
        }

        override fun onDraw(canvas: Canvas) {
            super.onDraw(canvas)
            canvas.save()
            canvas.scale(scale, scale)
            canvas.drawPath(arrow, fill)
            canvas.drawPath(arrow, outline)
            canvas.restore()
        }
    }

    companion object {
        private const val TAG = "MyKvmCursor"

        /** The arrow path above is authored inside this box. */
        private const val DESIGN_BOX = 40f

        /**
         * Bounds in pixels, applied after the density conversion done by the
         * caller. Wide enough to be a useful safety net, narrow enough that a
         * bad setting cannot cover the screen.
         */
        const val MIN_SIZE_PX = 16
        const val MAX_SIZE_PX = 200
    }
}