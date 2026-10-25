package io.github.redhatontop.iolite

import android.annotation.SuppressLint
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.hypot

/** The button that brings the key bar back. It can be dragged anywhere and rests on the nearer side. */
@SuppressLint("ClickableViewAccessibility")
class FloatingPill(private val view: View, private val prefs: Prefs, onClick: () -> Unit) {
    private val slop = ViewConfiguration.get(view.context).scaledTouchSlop
    private val margin = MARGIN_DP * view.resources.displayMetrics.density
    private var bottomInset = 0
    private var downX = 0f
    private var downY = 0f
    private var offsetX = 0f
    private var offsetY = 0f
    private var dragging = false

    init {
        view.setOnClickListener { onClick() }
        view.setOnTouchListener { v, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = e.rawX
                    downY = e.rawY
                    offsetX = v.x - e.rawX
                    offsetY = v.y - e.rawY
                    dragging = false
                    v.isPressed = true
                }
                MotionEvent.ACTION_MOVE -> {
                    if (!dragging && hypot(e.rawX - downX, e.rawY - downY) > slop) {
                        dragging = true
                        v.isPressed = false
                    }
                    if (dragging) {
                        v.x = (e.rawX + offsetX).coerceIn(0f, maxX())
                        v.y = (e.rawY + offsetY).coerceIn(minY(), maxY())
                    }
                }
                MotionEvent.ACTION_UP -> {
                    v.isPressed = false
                    if (dragging) settle() else v.performClick()
                }
                MotionEvent.ACTION_CANCEL -> v.isPressed = false
            }
            true
        }
        view.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> if (!dragging) place() }
    }

    /** Keeps the button above the keyboard when it is open. */
    fun setBottomInset(inset: Int) {
        if (inset == bottomInset) return
        bottomInset = inset
        place()
    }

    private fun place() {
        view.x = if (prefs.pillAtEnd) maxX() - margin else margin
        view.y = minY() + prefs.pillY * (maxY() - minY())
    }

    private fun settle() {
        val atEnd = view.x + view.width / 2f > parentWidth() / 2f
        val range = (maxY() - minY()).coerceAtLeast(1f)
        prefs.pillAtEnd = atEnd
        prefs.pillY = ((view.y - minY()) / range).coerceIn(0f, 1f)
        view.animate()
            .x(if (atEnd) maxX() - margin else margin)
            .setDuration(SETTLE_MS)
            .start()
    }

    private fun parentWidth(): Float = (view.parent as View).width.toFloat()

    private fun maxX(): Float = (parentWidth() - view.width).coerceAtLeast(0f)

    private fun minY(): Float = margin

    private fun maxY(): Float =
        ((view.parent as View).height - bottomInset - view.height - margin).coerceAtLeast(minY())

    private companion object {
        const val MARGIN_DP = 12f
        const val SETTLE_MS = 160L
    }
}
