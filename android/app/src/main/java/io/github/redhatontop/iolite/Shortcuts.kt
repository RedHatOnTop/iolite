package io.github.redhatontop.iolite

/** Keys pressed together in order and released in reverse. An empty name shows the keys. */
data class Shortcut(val keys: List<Int>, val name: String = "")

class Preset(val name: Int, vararg keys: Int) {
    val keys = keys.toList()
}

class PresetGroup(val title: Int, val presets: List<Preset>)

/**
 * GNOME's default bindings and the common application ones. Each group is laid out in columns of
 * two, so presets come in pairs that read top to bottom.
 */
object Presets {
    private const val CTRL = Keysyms.CTRL_LEFT
    private const val ALT = Keysyms.ALT_LEFT
    private const val SHIFT = Keysyms.SHIFT_LEFT
    private const val SUPER = Keysyms.META_LEFT

    val GROUPS = listOf(
        PresetGroup(
            R.string.group_desktop,
            listOf(
                Preset(R.string.sc_overview, SUPER),
                Preset(R.string.sc_apps, SUPER, 'a'.code),
                Preset(R.string.sc_switch, ALT, Keysyms.TAB),
                Preset(R.string.sc_close_window, ALT, Keysyms.F1 + 3),
                Preset(R.string.sc_maximize, SUPER, Keysyms.DPAD_UP),
                Preset(R.string.sc_hide_window, SUPER, 'h'.code),
                Preset(R.string.sc_left_half, SUPER, Keysyms.DPAD_LEFT),
                Preset(R.string.sc_right_half, SUPER, Keysyms.DPAD_RIGHT),
                Preset(R.string.sc_screenshot, Keysyms.PRINT),
                Preset(R.string.sc_lock, SUPER, 'l'.code),
            ),
        ),
        PresetGroup(
            R.string.group_edit,
            listOf(
                Preset(R.string.sc_copy, CTRL, 'c'.code),
                Preset(R.string.sc_paste, CTRL, 'v'.code),
                Preset(R.string.sc_cut, CTRL, 'x'.code),
                Preset(R.string.sc_select_all, CTRL, 'a'.code),
                Preset(R.string.sc_undo, CTRL, 'z'.code),
                Preset(R.string.sc_redo, CTRL, SHIFT, 'z'.code),
                Preset(R.string.sc_find, CTRL, 'f'.code),
                Preset(R.string.sc_save, CTRL, 's'.code),
            ),
        ),
        PresetGroup(
            R.string.group_terminal,
            listOf(
                Preset(R.string.sc_copy, CTRL, SHIFT, 'c'.code),
                Preset(R.string.sc_paste, CTRL, SHIFT, 'v'.code),
                Preset(R.string.sc_interrupt, CTRL, 'c'.code),
                Preset(R.string.sc_end_input, CTRL, 'd'.code),
                Preset(R.string.sc_new_tab, CTRL, SHIFT, 't'.code),
                Preset(R.string.sc_clear, CTRL, 'l'.code),
            ),
        ),
        PresetGroup(
            R.string.group_tabs,
            listOf(
                Preset(R.string.sc_new_tab, CTRL, 't'.code),
                Preset(R.string.sc_close_tab, CTRL, 'w'.code),
                Preset(R.string.sc_next_tab, CTRL, Keysyms.TAB),
                Preset(R.string.sc_previous_tab, CTRL, SHIFT, Keysyms.TAB),
                Preset(R.string.sc_reopen_tab, CTRL, SHIFT, 't'.code),
                Preset(R.string.sc_address, CTRL, 'l'.code),
                Preset(R.string.sc_reload, CTRL, 'r'.code),
                Preset(R.string.sc_actual_size, CTRL, '0'.code),
                Preset(R.string.sc_zoom_in, CTRL, '+'.code),
                Preset(R.string.sc_zoom_out, CTRL, '-'.code),
            ),
        ),
    )
}
