package io.github.redhatontop.iolite

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.ColorStateList
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.text.style.TypefaceSpan
import android.util.AttributeSet
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.widget.FrameLayout
import android.widget.GridLayout
import android.widget.HorizontalScrollView
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.Space
import android.widget.TextView
import kotlin.math.roundToInt

/**
 * The bar along the bottom of the stream: a strip of page tabs and controls over keys the phone's
 * keyboard does not have, the function keys, or shortcuts.
 */
class KeyBar @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : LinearLayout(context, attrs) {

    enum class Page(val title: Int) {
        KEYS(R.string.tab_keys),
        FUNCTION(R.string.tab_function),
        SHORTCUTS(R.string.tab_shortcuts),
    }

    lateinit var keyboard: KeyboardBridge
    var onKeyboard: () -> Unit = {}
    var onMore: (View) -> Unit = {}
    var onHide: () -> Unit = {}
    var onAddShortcut: () -> Unit = {}
    var onEditShortcut: (Int) -> Unit = {}
    var onPageChanged: () -> Unit = {}

    var page = Page.KEYS
        private set
    var expanded = true
        private set

    private val modifierKeys = mutableListOf<Pair<TextView, Int>>()
    private val keyboardButton = iconButton(R.drawable.ic_keyboard, R.string.keyboard) { onKeyboard() }
    private val tabs = Page.entries.associateWith(::tab)
    private val mine = grid()
    private val pages = mapOf(
        Page.KEYS to keyRows(KEYS_ROWS),
        Page.FUNCTION to keyRows(FUNCTION_ROWS),
        Page.SHORTCUTS to shortcutsPage(),
    )
    private val body = FrameLayout(context)

    init {
        orientation = VERTICAL
        addView(strip(), LayoutParams(LayoutParams.MATCH_PARENT, dp(STRIP_DP)))
        addView(
            View(context).apply { setBackgroundResource(R.color.hairline) },
            LayoutParams(LayoutParams.MATCH_PARENT, dp(1)),
        )
        // Pages that are not shown stay invisible rather than gone, so the bar keeps the height of
        // the tallest and the video above it does not jump when the page changes.
        pages.values.forEach {
            body.addView(it, FrameLayout.LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        }
        addView(body, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT))
        render()
    }

    fun restore(page: Page, expanded: Boolean) {
        this.page = page
        this.expanded = expanded
        render()
    }

    fun setKeyboardShown(shown: Boolean) {
        keyboardButton.imageTintList = ColorStateList.valueOf(
            context.getColor(if (shown) R.color.honey else R.color.text),
        )
    }

    fun setShortcuts(shortcuts: List<Shortcut>) {
        mine.removeAllViews()
        shortcuts.forEachIndexed { index, shortcut ->
            mine.addView(
                chip(comboText(shortcut.name, shortcut.keys), R.drawable.bg_chip_mine).apply {
                    setOnClickListener { send(it, shortcut.keys) }
                    setOnLongClickListener {
                        onEditShortcut(index)
                        true
                    }
                },
            )
        }
        mine.addView(
            chip("+  " + context.getString(R.string.add_shortcut), R.drawable.bg_chip_add).apply {
                setTextColor(context.getColor(R.color.text_dim))
                setOnClickListener { onAddShortcut() }
            },
        )
    }

    fun refreshModifiers() {
        for ((view, keysym) in modifierKeys) {
            val latch = keyboard.latch(keysym)
            view.setTextColor(
                context.getColor(
                    when (latch) {
                        null -> R.color.text
                        Latch.ONCE -> R.color.honey
                        Latch.LOCKED -> R.color.honey_ink
                    },
                ),
            )
            view.setBackgroundResource(
                when (latch) {
                    null -> R.drawable.bg_key
                    Latch.ONCE -> R.drawable.bg_key_active
                    Latch.LOCKED -> R.drawable.bg_key_locked
                },
            )
        }
    }

    private fun select(target: Page) {
        if (target == page && expanded) {
            expanded = false
        } else {
            page = target
            expanded = true
        }
        render()
        onPageChanged()
    }

    private fun render() {
        tabs.forEach { (tabPage, tab) ->
            val current = tabPage == page
            tab.isSelected = current
            tab.setTextColor(context.getColor(if (current) R.color.text else R.color.text_dim))
            tab.setBackgroundResource(if (current && expanded) R.drawable.bg_tab_active else R.drawable.bg_key)
        }
        body.visibility = if (expanded) VISIBLE else GONE
        pages.forEach { (pagePage, view) -> view.visibility = if (pagePage == page) VISIBLE else INVISIBLE }
    }

    private fun strip(): View = LinearLayout(context).apply {
        gravity = Gravity.CENTER_VERTICAL
        addView(keyboardButton, iconParams())
        tabs.values.forEach { addView(it, LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.MATCH_PARENT)) }
        addView(Space(context), LayoutParams(0, 0, 1f))
        addView(
            iconButton(R.drawable.ic_overview, R.string.sc_overview) { send(it, listOf(Keysyms.META_LEFT)) },
            iconParams(),
        )
        addView(iconButton(R.drawable.ic_more, R.string.more) { onMore(it) }, iconParams())
        addView(iconButton(R.drawable.ic_hide, R.string.hide_keys) { onHide() }, iconParams())
    }

    private fun tab(page: Page): TextView = TextView(context, null, 0, R.style.BarTab).apply {
        setText(page.title)
        setOnClickListener { select(page) }
    }

    private fun iconButton(icon: Int, description: Int, onClick: (View) -> Unit) =
        ImageButton(context, null, 0, R.style.BarIcon).apply {
            setImageResource(icon)
            contentDescription = context.getString(description)
            setOnClickListener(onClick)
        }

    private fun iconParams() = LayoutParams(dp(ICON_DP), LayoutParams.MATCH_PARENT)

    private fun keyRows(rows: List<List<Int>>): View = LinearLayout(context).apply {
        orientation = VERTICAL
        gravity = Gravity.CENTER_VERTICAL
        rows.forEach { row ->
            val line = LinearLayout(context)
            row.forEach { line.addView(key(it), LayoutParams(0, LayoutParams.MATCH_PARENT, 1f)) }
            addView(line, LayoutParams(LayoutParams.MATCH_PARENT, dp(KEY_ROW_DP)))
        }
    }

    private fun key(keysym: Int): TextView = TextView(context, null, 0, R.style.BarKey).apply {
        text = LEGENDS[keysym] ?: Keysyms.label(keysym)
        contentDescription = Keysyms.label(keysym)
        if (keysym in Keysyms.MODIFIERS) {
            modifierKeys += this to keysym
            setOnClickListener {
                it.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
                keyboard.tapModifier(keysym)
            }
            setOnLongClickListener {
                keyboard.lockModifier(keysym)
                true
            }
        } else {
            holdable(this, keysym)
        }
    }

    /** Down and up follow the finger, so holding an arrow or Backspace repeats on the computer. */
    @SuppressLint("ClickableViewAccessibility")
    private fun holdable(view: View, keysym: Int) {
        view.setOnClickListener { keyboard.sendShortcut(listOf(keysym)) }
        view.setOnTouchListener { v, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    v.background?.setHotspot(e.x, e.y)
                    v.isPressed = true
                    v.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
                    keyboard.keyDown(keysym)
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    v.isPressed = false
                    keyboard.keyUp(keysym)
                }
            }
            true
        }
    }

    private fun shortcutsPage(): View {
        val groups = LinearLayout(context).apply { setPadding(dp(4), 0, dp(8), 0) }
        groups.addView(group(R.string.group_mine, mine))
        Presets.GROUPS.forEach { presetGroup ->
            val grid = grid()
            presetGroup.presets.forEach { preset ->
                grid.addView(
                    chip(comboText(context.getString(preset.name), preset.keys), R.drawable.bg_chip).apply {
                        setOnClickListener { send(it, preset.keys) }
                    },
                )
            }
            groups.addView(group(presetGroup.title, grid))
        }
        return HorizontalScrollView(context).apply {
            isHorizontalScrollBarEnabled = false
            addView(groups)
        }
    }

    private fun grid() = GridLayout(context).apply {
        rowCount = 2
        orientation = GridLayout.VERTICAL
    }

    private fun group(title: Int, grid: GridLayout): View = LinearLayout(context).apply {
        orientation = VERTICAL
        setPadding(dp(6), dp(6), dp(6), dp(2))
        addView(
            TextView(context, null, 0, R.style.Caption).apply {
                setText(title)
                textSize = 10f
                includeFontPadding = false
            },
            LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                marginStart = dp(4)
                bottomMargin = dp(2)
            },
        )
        addView(grid)
    }

    private fun chip(text: CharSequence, background: Int): TextView = TextView(context).apply {
        this.text = text
        setTextColor(context.getColor(R.color.text))
        textSize = 13f
        maxLines = 1
        gravity = Gravity.CENTER_VERTICAL
        setPadding(dp(10), 0, dp(10), 0)
        setBackgroundResource(background)
        layoutParams = GridLayout.LayoutParams(
            GridLayout.spec(GridLayout.UNDEFINED),
            GridLayout.spec(GridLayout.UNDEFINED, GridLayout.FILL),
        ).apply {
            height = dp(CHIP_DP)
            setMargins(dp(2), dp(2), dp(2), dp(2))
        }
    }

    /** The name, then the keys in a quieter monospace; the keys alone when there is no name. */
    private fun comboText(name: String, keys: List<Int>): CharSequence = SpannableStringBuilder().apply {
        if (name.isNotEmpty()) append(name).append("  ")
        val start = length
        append(Keysyms.combo(keys))
        setSpan(TypefaceSpan("monospace"), start, length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        if (name.isNotEmpty()) {
            setSpan(ForegroundColorSpan(context.getColor(R.color.text_dim)), start, length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            setSpan(RelativeSizeSpan(0.85f), start, length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        }
    }

    private fun send(view: View, keys: List<Int>) {
        view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
        keyboard.sendShortcut(keys)
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).roundToInt()

    private companion object {
        const val STRIP_DP = 40
        const val ICON_DP = 44
        const val KEY_ROW_DP = 42
        const val CHIP_DP = 29

        val LEGENDS = mapOf(Keysyms.DEL to "⌫", Keysyms.ENTER to "↵", Keysyms.SPACE to "␣")

        /** The arrows sit in an inverted T with Page Up and Page Down beside the up arrow. */
        val KEYS_ROWS = listOf(
            listOf(
                Keysyms.ESCAPE, Keysyms.TAB, Keysyms.MOVE_HOME, Keysyms.MOVE_END, Keysyms.PAGE_UP,
                Keysyms.DPAD_UP, Keysyms.PAGE_DOWN, Keysyms.DEL, Keysyms.FORWARD_DEL,
            ),
            listOf(
                Keysyms.CTRL_LEFT, Keysyms.ALT_LEFT, Keysyms.SHIFT_LEFT, Keysyms.META_LEFT, Keysyms.DPAD_LEFT,
                Keysyms.DPAD_DOWN, Keysyms.DPAD_RIGHT, Keysyms.SPACE, Keysyms.ENTER,
            ),
        )

        /** The modifiers stay where they are on the first page, for Alt+F4 and the like. */
        val FUNCTION_ROWS = listOf(
            (0..7).map { Keysyms.F1 + it } + Keysyms.PRINT,
            listOf(Keysyms.CTRL_LEFT, Keysyms.ALT_LEFT, Keysyms.SHIFT_LEFT, Keysyms.META_LEFT) +
                (8..11).map { Keysyms.F1 + it } + Keysyms.INSERT,
        )
    }
}
