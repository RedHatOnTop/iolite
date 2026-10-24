package io.github.redhatontop.iolite

import android.os.Handler
import android.os.Looper
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import kotlin.math.max
import kotlin.math.sqrt

enum class InputMode { TOUCH, TRACKPAD }

class RemoteSession {
    @Volatile
    var handle = 0L
    var frameWidth = 1
    var frameHeight = 1
}

class TouchInput(
    private val view: View,
    private val remote: RemoteSession,
    var mode: InputMode,
    private val onInteract: () -> Unit,
) {
    private val handler = Handler(Looper.getMainLooper())
    private val slop = TAP_SLOP_DP * view.resources.displayMetrics.density
    private val longPress = Runnable { onLongPress() }

    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var downTime = 0L
    private var travel = 0f
    private var dragArmed = false
    private var dragging = false
    private var longPressFired = false
    private var multiTouch = false
    private var multiStart = 0L
    private var lastTapEnd = 0L
    private var centroidX = 0f
    private var centroidY = 0f
    private var centroidStartX = 0f
    private var centroidStartY = 0f
    private var centroidTravel = 0f
    private var scrolled = false

    fun attach() {
        view.setOnTouchListener { _, event -> onTouch(event) }
        view.setOnGenericMotionListener { _, event -> onGenericMotion(event) }
    }

    private fun onTouch(event: MotionEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            if (event.actionMasked == MotionEvent.ACTION_MOVE) pointerMove(event.x, event.y)
            return true
        }
        if (event.actionMasked == MotionEvent.ACTION_DOWN) onInteract()
        when (mode) {
            InputMode.TOUCH -> touchMode(event)
            InputMode.TRACKPAD -> trackpadMode(event)
        }
        return true
    }

    private fun onGenericMotion(event: MotionEvent): Boolean {
        if (!event.isFromSource(InputDevice.SOURCE_MOUSE)) return false
        when (event.actionMasked) {
            MotionEvent.ACTION_HOVER_MOVE -> pointerMove(event.x, event.y)
            MotionEvent.ACTION_BUTTON_PRESS -> mouseButton(event, true)
            MotionEvent.ACTION_BUTTON_RELEASE -> mouseButton(event, false)
            MotionEvent.ACTION_SCROLL -> Native.scroll(
                remote.handle,
                -event.getAxisValue(MotionEvent.AXIS_HSCROLL) * MOUSE_SCROLL,
                -event.getAxisValue(MotionEvent.AXIS_VSCROLL) * MOUSE_SCROLL,
                true,
            )
            else -> return false
        }
        return true
    }

    private fun touchMode(event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN ->
                sendTouch(Native.TOUCH_DOWN, event, event.actionIndex)
            MotionEvent.ACTION_MOVE ->
                for (i in 0 until event.pointerCount) sendTouch(Native.TOUCH_MOVE, event, i)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP ->
                sendTouch(Native.TOUCH_UP, event, event.actionIndex)
            MotionEvent.ACTION_CANCEL ->
                for (i in 0 until event.pointerCount) sendTouch(Native.TOUCH_CANCEL, event, i)
        }
    }

    private fun trackpadMode(event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> beginGesture(event)
            MotionEvent.ACTION_POINTER_DOWN -> if (event.pointerCount == 2) beginMultiTouch(event)
            MotionEvent.ACTION_MOVE -> when {
                multiTouch -> multiTouchMove(event)
                event.pointerCount == 1 -> oneFingerMove(event)
            }
            MotionEvent.ACTION_POINTER_UP -> if (multiTouch && event.pointerCount == 2) endMultiTouch(event)
            MotionEvent.ACTION_UP -> endGesture(event)
            MotionEvent.ACTION_CANCEL -> cancelGesture()
        }
    }

    private fun beginGesture(event: MotionEvent) {
        handler.removeCallbacks(longPress)
        downX = event.x
        downY = event.y
        lastX = downX
        lastY = downY
        downTime = event.eventTime
        travel = 0f
        dragArmed = downTime - lastTapEnd < TAP_DRAG_MS
        dragging = false
        longPressFired = false
        multiTouch = false
        scrolled = false
        handler.postDelayed(longPress, LONG_PRESS_MS)
    }

    private fun oneFingerMove(event: MotionEvent) {
        val x = event.x
        val y = event.y
        travel = max(travel, distance(downX, downY, x, y))
        if (travel > slop) {
            handler.removeCallbacks(longPress)
            if (dragArmed && !dragging) {
                dragging = true
                Native.button(remote.handle, Native.BUTTON_LEFT, true)
            }
        }
        val scale = GAIN * pixelScale()
        Native.pointerRelative(remote.handle, (x - lastX) * scale, (y - lastY) * scale)
        lastX = x
        lastY = y
    }

    private fun beginMultiTouch(event: MotionEvent) {
        handler.removeCallbacks(longPress)
        if (dragging) {
            dragging = false
            Native.button(remote.handle, Native.BUTTON_LEFT, false)
        }
        dragArmed = false
        multiTouch = true
        multiStart = event.eventTime
        val (cx, cy) = centroid(event)
        centroidX = cx
        centroidY = cy
        centroidStartX = cx
        centroidStartY = cy
        centroidTravel = 0f
    }

    private fun multiTouchMove(event: MotionEvent) {
        if (event.pointerCount < 2) return
        val (cx, cy) = centroid(event)
        centroidTravel = max(centroidTravel, distance(centroidStartX, centroidStartY, cx, cy))
        if (centroidTravel <= slop) return
        val scale = pixelScale()
        Native.scroll(remote.handle, -(cx - centroidX) * scale, -(cy - centroidY) * scale, false)
        centroidX = cx
        centroidY = cy
        scrolled = true
    }

    private fun endMultiTouch(event: MotionEvent) {
        if (scrolled) Native.scroll(remote.handle, 0f, 0f, true)
        if (!scrolled && centroidTravel <= slop && event.eventTime - multiStart < TAP_MS) {
            click(Native.BUTTON_RIGHT)
        }
        scrolled = false
    }

    private fun endGesture(event: MotionEvent) {
        handler.removeCallbacks(longPress)
        if (multiTouch) {
            if (scrolled) Native.scroll(remote.handle, 0f, 0f, true)
            multiTouch = false
            scrolled = false
            return
        }
        if (dragging) {
            dragging = false
            Native.button(remote.handle, Native.BUTTON_LEFT, false)
        } else if (!longPressFired && travel <= slop && event.eventTime - downTime < TAP_MS) {
            click(Native.BUTTON_LEFT)
            lastTapEnd = event.eventTime
        }
        dragArmed = false
    }

    private fun cancelGesture() {
        handler.removeCallbacks(longPress)
        if (dragging) Native.button(remote.handle, Native.BUTTON_LEFT, false)
        if (scrolled) Native.scroll(remote.handle, 0f, 0f, true)
        dragging = false
        dragArmed = false
        multiTouch = false
        scrolled = false
    }

    private fun onLongPress() {
        if (multiTouch || dragging || travel > slop) return
        longPressFired = true
        click(Native.BUTTON_RIGHT)
    }

    private fun sendTouch(phase: Int, event: MotionEvent, index: Int) {
        Native.touch(
            remote.handle,
            phase,
            event.getPointerId(index),
            normalizeX(event.getX(index)),
            normalizeY(event.getY(index)),
        )
    }

    private fun pointerMove(x: Float, y: Float) {
        Native.pointerMove(remote.handle, normalizeX(x), normalizeY(y))
    }

    private fun mouseButton(event: MotionEvent, down: Boolean) {
        val button = when (event.actionButton) {
            MotionEvent.BUTTON_SECONDARY -> Native.BUTTON_RIGHT
            MotionEvent.BUTTON_TERTIARY -> Native.BUTTON_MIDDLE
            else -> Native.BUTTON_LEFT
        }
        Native.button(remote.handle, button, down)
    }

    private fun click(button: Int) {
        Native.button(remote.handle, button, true)
        Native.button(remote.handle, button, false)
    }

    private fun centroid(event: MotionEvent): Pair<Float, Float> {
        var sumX = 0f
        var sumY = 0f
        for (i in 0 until event.pointerCount) {
            sumX += event.getX(i)
            sumY += event.getY(i)
        }
        return sumX / event.pointerCount to sumY / event.pointerCount
    }

    private fun pixelScale(): Float = remote.frameWidth.toFloat() / view.width.coerceAtLeast(1)

    private fun normalizeX(x: Float): Float = (x / view.width.coerceAtLeast(1)).coerceIn(0f, 1f)

    private fun normalizeY(y: Float): Float = (y / view.height.coerceAtLeast(1)).coerceIn(0f, 1f)

    private fun distance(x1: Float, y1: Float, x2: Float, y2: Float): Float {
        val dx = x2 - x1
        val dy = y2 - y1
        return sqrt(dx * dx + dy * dy)
    }

    private companion object {
        const val TAP_SLOP_DP = 12f
        const val TAP_MS = 200L
        const val TAP_DRAG_MS = 300L
        const val LONG_PRESS_MS = 500L
        const val GAIN = 1.4f
        const val MOUSE_SCROLL = 40f
    }
}
