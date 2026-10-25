package io.github.redhatontop.iolite

import android.view.KeyEvent

object Keysyms {
    const val SPACE = 0x20
    const val DEL = 0xff08
    const val TAB = 0xff09
    const val ENTER = 0xff0d
    const val ESCAPE = 0xff1b
    const val MOVE_HOME = 0xff50
    const val DPAD_LEFT = 0xff51
    const val DPAD_UP = 0xff52
    const val DPAD_RIGHT = 0xff53
    const val DPAD_DOWN = 0xff54
    const val PAGE_UP = 0xff55
    const val PAGE_DOWN = 0xff56
    const val MOVE_END = 0xff57
    const val PRINT = 0xff61
    const val INSERT = 0xff63
    const val F1 = 0xffbe
    const val F12 = 0xffc9
    const val CAPS_LOCK = 0xffe5
    const val SHIFT_LEFT = 0xffe1
    const val SHIFT_RIGHT = 0xffe2
    const val CTRL_LEFT = 0xffe3
    const val CTRL_RIGHT = 0xffe4
    const val ALT_LEFT = 0xffe9
    const val ALT_RIGHT = 0xffea
    const val META_LEFT = 0xffeb
    const val META_RIGHT = 0xffec
    const val FORWARD_DEL = 0xffff

    val MODIFIERS = setOf(
        SHIFT_LEFT, SHIFT_RIGHT, CTRL_LEFT, CTRL_RIGHT, ALT_LEFT, ALT_RIGHT, META_LEFT, META_RIGHT,
    )

    private const val UNICODE_BASE = 0x01000000

    /** Names a shortcut can spell a key with; the first name of each key is how it is shown. */
    private val NAMES = listOf(
        "Ctrl" to CTRL_LEFT, "Control" to CTRL_LEFT,
        "Alt" to ALT_LEFT,
        "Shift" to SHIFT_LEFT,
        "Super" to META_LEFT, "Win" to META_LEFT, "Meta" to META_LEFT,
        "Esc" to ESCAPE, "Escape" to ESCAPE,
        "Tab" to TAB,
        "Enter" to ENTER, "Return" to ENTER,
        "Space" to SPACE,
        "Backspace" to DEL,
        "Del" to FORWARD_DEL, "Delete" to FORWARD_DEL,
        "Ins" to INSERT, "Insert" to INSERT,
        "Home" to MOVE_HOME,
        "End" to MOVE_END,
        "PgUp" to PAGE_UP, "PageUp" to PAGE_UP,
        "PgDn" to PAGE_DOWN, "PageDown" to PAGE_DOWN,
        "←" to DPAD_LEFT, "Left" to DPAD_LEFT,
        "↑" to DPAD_UP, "Up" to DPAD_UP,
        "→" to DPAD_RIGHT, "Right" to DPAD_RIGHT,
        "↓" to DPAD_DOWN, "Down" to DPAD_DOWN,
        "PrtSc" to PRINT, "Print" to PRINT,
    ) + (1..12).map { "F$it" to F1 + it - 1 }

    /** Dubeolsik keys for the compatibility jamo from U+3131; blank where none is a single key. */
    private const val JAMO_KEYS = "rR s  eEf       aqQ tTdwWczxvgkoiOjpuPh   yn   bm l"
    private const val JAMO_BASE = 0x3131

    fun forKeyEvent(e: KeyEvent): Int? {
        named(e.keyCode)?.let { return it }
        val codePoint = e.getUnicodeChar(0)
        return if (codePoint == 0) null else forCodePoint(codePoint)
    }

    /** Keysym for a shortcut, reading a Korean jamo as the Latin key it sits on. */
    fun forShortcut(codePoint: Int): Int? {
        val key = JAMO_KEYS.getOrNull(codePoint - JAMO_BASE)?.takeIf { it != ' ' }
        return forCodePoint(key?.code ?: codePoint)
    }

    fun label(keysym: Int): String {
        NAMES.firstOrNull { it.second == keysym }?.let { return it.first }
        val codePoint = if (keysym >= UNICODE_BASE) keysym - UNICODE_BASE else keysym
        return String(Character.toChars(codePoint)).uppercase()
    }

    fun combo(keys: List<Int>): String = keys.joinToString("+", transform = ::label)

    /** A key name such as `F5` or `Enter`, or a single character, which may be a jamo. */
    fun parse(name: String): Int? {
        val text = name.trim()
        NAMES.firstOrNull { it.first.equals(text, ignoreCase = true) }?.let { return it.second }
        val codePoint = text.codePoints().toArray().singleOrNull() ?: return null
        return forShortcut(Character.toLowerCase(codePoint))
    }

    fun forCodePoint(codePoint: Int): Int? = when (codePoint) {
        in 0x20..0x7e, in 0xa0..0xff -> codePoint
        in 0x100..Int.MAX_VALUE -> UNICODE_BASE + codePoint
        else -> null
    }

    private fun named(keyCode: Int): Int? = when (keyCode) {
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> ENTER
        KeyEvent.KEYCODE_DEL -> DEL
        KeyEvent.KEYCODE_FORWARD_DEL -> FORWARD_DEL
        KeyEvent.KEYCODE_TAB -> TAB
        KeyEvent.KEYCODE_ESCAPE -> ESCAPE
        KeyEvent.KEYCODE_DPAD_LEFT -> DPAD_LEFT
        KeyEvent.KEYCODE_DPAD_UP -> DPAD_UP
        KeyEvent.KEYCODE_DPAD_RIGHT -> DPAD_RIGHT
        KeyEvent.KEYCODE_DPAD_DOWN -> DPAD_DOWN
        KeyEvent.KEYCODE_MOVE_HOME -> MOVE_HOME
        KeyEvent.KEYCODE_MOVE_END -> MOVE_END
        KeyEvent.KEYCODE_PAGE_UP -> PAGE_UP
        KeyEvent.KEYCODE_PAGE_DOWN -> PAGE_DOWN
        KeyEvent.KEYCODE_INSERT -> INSERT
        KeyEvent.KEYCODE_SYSRQ -> PRINT
        in KeyEvent.KEYCODE_F1..KeyEvent.KEYCODE_F12 -> F1 + (keyCode - KeyEvent.KEYCODE_F1)
        KeyEvent.KEYCODE_CAPS_LOCK -> CAPS_LOCK
        KeyEvent.KEYCODE_SHIFT_LEFT -> SHIFT_LEFT
        KeyEvent.KEYCODE_SHIFT_RIGHT -> SHIFT_RIGHT
        KeyEvent.KEYCODE_CTRL_LEFT -> CTRL_LEFT
        KeyEvent.KEYCODE_CTRL_RIGHT -> CTRL_RIGHT
        KeyEvent.KEYCODE_ALT_LEFT -> ALT_LEFT
        KeyEvent.KEYCODE_ALT_RIGHT -> ALT_RIGHT
        KeyEvent.KEYCODE_META_LEFT -> META_LEFT
        KeyEvent.KEYCODE_META_RIGHT -> META_RIGHT
        KeyEvent.KEYCODE_SPACE -> SPACE
        else -> null
    }
}
