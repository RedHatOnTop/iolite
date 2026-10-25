package io.github.redhatontop.iolite

import android.content.Context
import android.util.AttributeSet
import android.view.View
import android.widget.FrameLayout
import kotlin.math.roundToInt

class AspectFrame @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : FrameLayout(context, attrs) {

    private var aspect = 0f

    fun setAspect(width: Int, height: Int) {
        aspect = if (width > 0 && height > 0) width.toFloat() / height else 0f
        requestLayout()
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val maxWidth = View.MeasureSpec.getSize(widthMeasureSpec)
        val maxHeight = View.MeasureSpec.getSize(heightMeasureSpec)
        setMeasuredDimension(maxWidth, maxHeight)
        val child = getChildAt(0) ?: return
        val (width, height) = fit(
            (maxWidth - paddingLeft - paddingRight).coerceAtLeast(1),
            (maxHeight - paddingTop - paddingBottom).coerceAtLeast(1),
        )
        child.measure(
            View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY),
        )
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        val child = getChildAt(0) ?: return
        val x = paddingLeft + (width - paddingLeft - paddingRight - child.measuredWidth) / 2
        val y = paddingTop + (height - paddingTop - paddingBottom - child.measuredHeight) / 2
        child.layout(x, y, x + child.measuredWidth, y + child.measuredHeight)
    }

    private fun fit(maxWidth: Int, maxHeight: Int): Pair<Int, Int> {
        if (aspect <= 0f) return maxWidth to maxHeight
        val heightForWidth = (maxWidth / aspect).roundToInt()
        return if (heightForWidth <= maxHeight) {
            maxWidth to heightForWidth
        } else {
            (maxHeight * aspect).roundToInt() to maxHeight
        }
    }
}
