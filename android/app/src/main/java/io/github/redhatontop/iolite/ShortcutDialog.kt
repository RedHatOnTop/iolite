package io.github.redhatontop.iolite

import android.app.Activity
import android.app.Dialog
import android.os.Handler
import android.os.Looper
import android.text.Editable
import android.text.TextWatcher
import android.view.View
import android.view.ViewGroup
import android.view.Window
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/** Builds a shortcut from held modifiers and one key; [existing] is edited, or a new one is made. */
class ShortcutDialog(
    private val activity: Activity,
    private val existing: Shortcut?,
    private val onSave: (Shortcut) -> Unit,
    private val onDelete: () -> Unit,
) {
    private val dialog = Dialog(activity)
    private val view = activity.layoutInflater.inflate(R.layout.dialog_shortcut, null)
    private val handler = Handler(Looper.getMainLooper())
    private val disarmDelete = Runnable { armDelete(false) }

    /** The key the shortcut ends on; a modifier itself when that is all it presses, like Super. */
    private val initialKey = existing?.keys?.let { keys -> keys.lastOrNull { it !in MODIFIERS } ?: keys.lastOrNull() }
    private val held = existing?.keys.orEmpty().filter { it in MODIFIERS && it != initialKey }.toMutableSet()
    private val toggles = mutableMapOf<Int, TextView>()
    private var deleteArmed = false

    private val keyInput: EditText = view.findViewById(R.id.keyInput)
    private val nameInput: EditText = view.findViewById(R.id.nameInput)
    private val preview: TextView = view.findViewById(R.id.preview)
    private val deleteButton: Button = view.findViewById(R.id.deleteButton)
    private val deleteBackground = deleteButton.background

    fun show() {
        view.findViewById<TextView>(R.id.dialogTitle)
            .setText(if (existing == null) R.string.new_shortcut else R.string.edit_shortcut)
        val row = view.findViewById<LinearLayout>(R.id.modifierToggles)
        MODIFIERS.forEach { keysym ->
            val toggle = activity.layoutInflater.inflate(R.layout.item_segment, row, false) as TextView
            toggle.text = Keysyms.label(keysym)
            toggle.setOnClickListener {
                if (!held.remove(keysym)) held.add(keysym)
                render()
            }
            toggles[keysym] = toggle
            row.addView(toggle)
        }
        initialKey?.let { keyInput.setText(Keysyms.label(it)) }
        existing?.let { nameInput.setText(it.name) }
        keyInput.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = Unit
            override fun afterTextChanged(s: Editable?) = render()
        })
        deleteButton.visibility = if (existing == null) View.GONE else View.VISIBLE
        deleteButton.setOnClickListener {
            if (deleteArmed) {
                dialog.dismiss()
                onDelete()
            } else {
                armDelete(true)
            }
        }
        view.findViewById<View>(R.id.cancelButton).setOnClickListener { dialog.dismiss() }
        view.findViewById<View>(R.id.saveButton).setOnClickListener { save() }
        render()

        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE)
        dialog.setContentView(view)
        dialog.setOnDismissListener { handler.removeCallbacks(disarmDelete) }
        dialog.show()
        dialog.window?.apply {
            setBackgroundDrawableResource(android.R.color.transparent)
            setLayout(dialogWidth(activity), ViewGroup.LayoutParams.WRAP_CONTENT)
        }
    }

    /** The modifiers in the usual order, then the key, or null with the reason shown. */
    private fun keys(): List<Int>? {
        val text = keyInput.text.toString()
        val key = Keysyms.parse(text)
        preview.setTextColor(activity.getColor(if (key == null) R.color.text_dim else R.color.honey))
        if (key == null) {
            preview.setText(if (text.isBlank()) R.string.shortcut_key_missing else R.string.shortcut_key_unknown)
            return null
        }
        return (MODIFIERS.filter { it in held } + key).distinct()
    }

    private fun render() {
        toggles.forEach { (keysym, toggle) -> styleSegment(toggle, keysym in held) }
        keys()?.let { preview.text = Keysyms.combo(it) }
    }

    private fun armDelete(armed: Boolean) {
        deleteArmed = armed
        handler.removeCallbacks(disarmDelete)
        if (armed) handler.postDelayed(disarmDelete, DELETE_ARMED_MS)
        deleteButton.setText(if (armed) R.string.delete_confirm else R.string.delete)
        deleteButton.setTextColor(activity.getColor(if (armed) R.color.honey_ink else R.color.danger))
        deleteButton.background = if (armed) activity.getDrawable(R.drawable.bg_button_danger) else deleteBackground
    }

    private fun save() {
        val keys = keys() ?: return
        dialog.dismiss()
        onSave(Shortcut(keys, nameInput.text.toString().trim()))
    }

    private companion object {
        val MODIFIERS = listOf(Keysyms.CTRL_LEFT, Keysyms.ALT_LEFT, Keysyms.SHIFT_LEFT, Keysyms.META_LEFT)
        const val DELETE_ARMED_MS = 3000L
    }
}
