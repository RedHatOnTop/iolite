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
    const val INSERT = 0xff63
    const val F1 = 0xffbe
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

    private const val UNICODE_BASE = 0x01000000

    fun forKeyEvent(e: KeyEvent): Int? {
        named(e.keyCode)?.let { return it }
        val codePoint = e.getUnicodeChar(0)
        return if (codePoint == 0) null else forCodePoint(codePoint)
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
