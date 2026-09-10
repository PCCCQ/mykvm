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
 * Android has no system cursor for a touch device, so the receiver draws its
 * own: a small always-on-top window whose *tip* tracks the coordinates the
 * desktop sends. Mouse movement never reaches the injected input stream -- only
 * clicks, drags and scrolls do -- which is what keeps hovering free of jank.
 *
 * Every WindowManager call and every View construction happens on the main
 * thread. `WindowManager.addView` builds a `ViewRootImpl`, which installs a
 * `Handler` on the calling thread, so calling it from the poll thread fails with
 * "Can't create handler inside thread that has not called Looper.prepare()".
 * Positions are also coalesced: a burst of mouse moves costs one relayout per
 * main-thread pass rather than one per event.
 */
class VirtualCursor(private val context: Context) {

    private val handler = Handler(Looper.getMainLooper())
    private val windowManager = context.getSystemService(WindowManager::class.java)

    /** Only ever touched on the main thread. */
    private var view: CursorView? = null
    private var layoutParams: WindowManager.LayoutParams? = null

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

    private fun scheduleApply() {
        if (applyScheduled) return
        applyScheduled = true
        handler.post(apply)
    }

    private fun applyOnMainThread() {
        if (!visible) {
            view?.let { current ->
                try {
                    windowManager.removeView(current)
                } catch (error: Throwable) {
                    // Already detached.
                }
            }
            view = null
            layoutParams = null
            return
        }

        val x = pendingX
        val y = pendingY
        val existing = view

        if (existing == null) {
            // Constructed here, not in the constructor, so the View (which
            // touches a Handler internally) is always built on the main thread.
            val created = CursorView(context)
            val params = WindowManager.LayoutParams(
                SIZE_PX,
                SIZE_PX,
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
            } catch (error: Throwable) {
                Log.w(TAG, "cannot show cursor overlay: ${error.message}")
                view = null
                layoutParams = null
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
        }
    }

    /** Draws an arrow whose tip sits at the window's top-left corner. */
    private class CursorView(context: Context) : View(context) {

        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.WHITE
            style = Paint.Style.FILL
        }
        private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.argb(220, 0, 0, 0)
            style = Paint.Style.STROKE
            strokeWidth = 3f
            strokeJoin = Paint.Join.ROUND
        }
        private val arrow = Path().apply {
            moveTo(1f, 1f)
            lineTo(1f, 30f)
            lineTo(9f, 23f)
            lineTo(15f, 35f)
            lineTo(20f, 32f)
            lineTo(14f, 21f)
            lineTo(24f, 20f)
            close()
        }

        override fun onDraw(canvas: Canvas) {
            super.onDraw(canvas)
            canvas.drawPath(arrow, fill)
            canvas.drawPath(arrow, outline)
        }
    }

    private companion object {
        const val TAG = "MyKvmCursor"

        /** Generous box: the arrow tip is at (0,0) and the tail extends below. */
        const val SIZE_PX = 48
    }
}