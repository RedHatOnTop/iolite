package io.github.redhatontop.iolite

import android.app.Activity
import android.app.Dialog
import android.os.Handler
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.view.Window
import android.widget.Button
import android.widget.EditText
import android.widget.Switch
import android.widget.TextView
import kotlin.math.min

/** Edits a saved computer: its name, address, input mode, sound and pairing. Nothing is saved until Save. */
class ComputerDialog(
    private val activity: Activity,
    private val prefs: Prefs,
    private val computer: Computer,
    private val onChanged: () -> Unit,
) {
    private val dialog = Dialog(activity)
    private val view = activity.layoutInflater.inflate(R.layout.dialog_computer, null)
    private val handler = Handler(Looper.getMainLooper())
    private val disarmDelete = Runnable { armDelete(false) }
    private var inputMode = computer.inputMode
    private var pin = computer.pin
    private var deleteArmed = false

    private val nameInput: EditText = view.findViewById(R.id.nameInput)
    private val hostInput: EditText = view.findViewById(R.id.hostInput)
    private val portInput: EditText = view.findViewById(R.id.portInput)
    private val touch: TextView = view.findViewById(R.id.modeTouch)
    private val trackpad: TextView = view.findViewById(R.id.modeTrackpad)
    private val sound: Switch = view.findViewById(R.id.soundSwitch)
    private val pairingText: TextView = view.findViewById(R.id.pairingText)
    private val forgetButton: Button = view.findViewById(R.id.forgetButton)
    private val deleteButton: Button = view.findViewById(R.id.deleteButton)
    private val deleteBackground = deleteButton.background

    fun show() {
        view.findViewById<TextView>(R.id.dialogTitle).text = computer.title
        nameInput.setText(computer.name)
        nameInput.hint = computer.hostName.ifEmpty { computer.endpoint.host }
        hostInput.setText(computer.endpoint.host)
        portInput.setText(computer.endpoint.port.toString())
        sound.isChecked = computer.audio
        touch.setOnClickListener { setInputMode(InputMode.TOUCH) }
        trackpad.setOnClickListener { setInputMode(InputMode.TRACKPAD) }
        forgetButton.setOnClickListener {
            pin = null
            renderPairing(forgotten = true)
        }
        deleteButton.setOnClickListener { if (deleteArmed) delete() else armDelete(true) }
        view.findViewById<View>(R.id.cancelButton).setOnClickListener { dialog.dismiss() }
        view.findViewById<View>(R.id.saveButton).setOnClickListener { save() }
        setInputMode(inputMode)
        renderPairing(forgotten = false)

        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE)
        dialog.setContentView(view)
        dialog.setOnDismissListener { handler.removeCallbacks(disarmDelete) }
        dialog.show()
        dialog.window?.apply {
            setBackgroundDrawableResource(android.R.color.transparent)
            setLayout(dialogWidth(activity), ViewGroup.LayoutParams.WRAP_CONTENT)
        }
    }

    private fun setInputMode(mode: InputMode) {
        inputMode = mode
        styleSegment(touch, mode == InputMode.TOUCH)
        styleSegment(trackpad, mode == InputMode.TRACKPAD)
    }

    private fun renderPairing(forgotten: Boolean) {
        val current = pin
        pairingText.text = when {
            current != null -> activity.getString(R.string.paired_with, current.take(16).chunked(4).joinToString(" "))
            forgotten -> activity.getString(R.string.pairing_forgotten)
            else -> activity.getString(R.string.pairing_none)
        }
        forgetButton.visibility = if (current != null) View.VISIBLE else View.GONE
    }

    private fun armDelete(armed: Boolean) {
        deleteArmed = armed
        handler.removeCallbacks(disarmDelete)
        if (armed) handler.postDelayed(disarmDelete, DELETE_ARMED_MS)
        deleteButton.setText(if (armed) R.string.delete_confirm else R.string.delete)
        deleteButton.setTextColor(activity.getColor(if (armed) R.color.honey_ink else R.color.danger))
        deleteButton.background = if (armed) activity.getDrawable(R.drawable.bg_button_danger) else deleteBackground
    }

    private fun delete() {
        prefs.delete(computer.id)
        dialog.dismiss()
        onChanged()
    }

    private fun save() {
        val endpoint = readEndpoint(hostInput, portInput) ?: return
        prefs.update(computer.id) {
            it.copy(
                endpoint = endpoint,
                name = nameInput.text.toString().trim(),
                pin = pin,
                inputMode = inputMode,
                audio = sound.isChecked,
            )
        }
        dialog.dismiss()
        onChanged()
    }

    private companion object {
        const val DELETE_ARMED_MS = 3000L
    }
}

fun styleSegment(segment: TextView, on: Boolean) {
    segment.isSelected = on
    segment.setBackgroundResource(if (on) R.drawable.bg_segment_on else R.drawable.bg_segment_off)
    segment.setTextColor(segment.context.getColor(if (on) R.color.honey_ink else R.color.text))
}

/** The screen's width less a margin, up to a comfortable reading width. */
fun dialogWidth(activity: Activity): Int {
    val density = activity.resources.displayMetrics.density
    val screen = activity.windowManager.currentWindowMetrics.bounds.width()
    return min(screen - (32 * density).toInt(), (440 * density).toInt())
}
