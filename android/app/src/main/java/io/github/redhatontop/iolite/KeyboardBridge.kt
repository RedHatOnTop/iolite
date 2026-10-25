package io.github.redhatontop.iolite

import android.content.Context
import android.text.InputType
import android.util.AttributeSet
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager

/** How a modifier key on the key bar is latched. */
enum class Latch {
    /** For the next key, click or shortcut. */
    ONCE,

    /** Held down on the computer until it is tapped again. */
    LOCKED,
}

class KeyboardBridge @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs), PointerModifiers {

    lateinit var remote: RemoteSession
    var onLatchChanged: (() -> Unit)? = null

    private val latches = linkedMapOf<Int, Latch>()
    private val heldWithKey = mutableMapOf<Int, List<Int>>()
    private var heldWithPointer: List<Int>? = null
    private var composing = ""

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI
        return Connection(this)
    }

    fun show() {
        requestFocus()
        context.getSystemService(InputMethodManager::class.java)
            .showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hide() {
        context.getSystemService(InputMethodManager::class.java)
            .hideSoftInputFromWindow(windowToken, 0)
    }

    fun latch(keysym: Int): Latch? = latches[keysym]

    /** Latches a modifier for the next action, or releases it. */
    fun tapModifier(keysym: Int) {
        when (latches.remove(keysym)) {
            null -> latches[keysym] = Latch.ONCE
            Latch.LOCKED -> key(keysym, false)
            Latch.ONCE -> Unit
        }
        onLatchChanged?.invoke()
    }

    fun lockModifier(keysym: Int) {
        if (latches[keysym] == Latch.LOCKED) return
        latches[keysym] = Latch.LOCKED
        key(keysym, true)
        onLatchChanged?.invoke()
    }

    /** Lets go of every latch; a new session starts with nothing held. */
    fun resetLatches() {
        latches.filterValues { it == Latch.LOCKED }.keys.forEach { key(it, false) }
        heldWithKey.values.forEach(::release)
        heldWithPointer?.let(::release)
        heldWithKey.clear()
        heldWithPointer = null
        if (latches.isNotEmpty()) {
            latches.clear()
            onLatchChanged?.invoke()
        }
    }

    /** Presses [keys] together with the latched modifiers, then releases them in reverse. */
    fun sendShortcut(keys: List<Int>) {
        finishComposition()
        pressLatched(keys)
    }

    /** A key bar key goes down with the latched modifiers and stays down until [keyUp], so it repeats. */
    fun keyDown(keysym: Int) {
        if (keysym in heldWithKey) return
        finishComposition()
        heldWithKey[keysym] = takeOnce()
        key(keysym, true)
    }

    fun keyUp(keysym: Int) {
        val modifiers = heldWithKey.remove(keysym) ?: return
        key(keysym, false)
        release(modifiers)
    }

    override fun pointerDown() {
        if (heldWithPointer == null) heldWithPointer = takeOnce()
    }

    override fun pointerUp() {
        heldWithPointer?.let(::release)
        heldWithPointer = null
    }

    fun key(keysym: Int, down: Boolean) {
        Native.key(remote.handle, keysym, down)
    }

    /** Commits the composing text, so a tap or a key lands after it on the computer. */
    fun finishComposition() {
        if (composing.isEmpty()) return
        commit(composing)
        restartInput()
    }

    /** Presses the one-shot modifiers and unlatches them; the caller releases them after its action. */
    private fun takeOnce(): List<Int> {
        val once = latches.filterValues { it == Latch.ONCE }.keys.toList()
        if (once.isEmpty()) return once
        once.forEach {
            latches.remove(it)
            key(it, true)
        }
        onLatchChanged?.invoke()
        return once
    }

    private fun pressLatched(keys: List<Int>) {
        val once = takeOnce()
        val down = keys.filter { latches[it] != Latch.LOCKED && it !in once }
        down.forEach { key(it, true) }
        down.asReversed().forEach { key(it, false) }
        release(once)
    }

    private fun release(modifiers: List<Int>) {
        modifiers.asReversed().forEach { key(it, false) }
    }

    private fun commit(text: String) {
        if (text.isNotEmpty() || composing.isNotEmpty()) Native.text(remote.handle, text)
        composing = ""
    }

    private fun compose(text: String) {
        if (text == composing) return
        composing = text
        Native.compose(remote.handle, text)
    }

    private fun shortcut(text: CharSequence): Int? {
        if (latches.isEmpty() || composing.isNotEmpty()) return null
        return text.codePoints().toArray().singleOrNull()?.let(Keysyms::forShortcut)
    }

    private fun restartInput() {
        context.getSystemService(InputMethodManager::class.java).restartInput(this)
    }

    private inner class Connection(view: View) : BaseInputConnection(view, false) {
        override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
            val keysym = shortcut(text)
            if (keysym != null) pressLatched(listOf(keysym)) else commit(text.toString())
            return true
        }

        override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
            val keysym = shortcut(text)
            if (keysym != null) {
                pressLatched(listOf(keysym))
                // Otherwise the input method keeps composing on top of the jamo it just sent.
                restartInput()
            } else {
                compose(text.toString())
            }
            return true
        }

        override fun finishComposingText(): Boolean {
            if (composing.isNotEmpty()) commit(composing)
            return true
        }

        override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
            repeat(beforeLength.coerceIn(0, MAX_DELETE)) { pressLatched(listOf(Keysyms.DEL)) }
            repeat(afterLength.coerceIn(0, MAX_DELETE)) { pressLatched(listOf(Keysyms.FORWARD_DEL)) }
            return true
        }

        override fun sendKeyEvent(event: KeyEvent): Boolean {
            val keysym = Keysyms.forKeyEvent(event) ?: return true
            when {
                keysym in Keysyms.MODIFIERS || Latch.ONCE !in latches.values ->
                    key(keysym, event.action == KeyEvent.ACTION_DOWN)
                event.action == KeyEvent.ACTION_DOWN -> pressLatched(listOf(keysym))
            }
            return true
        }
    }

    private companion object {
        const val MAX_DELETE = 64
    }
}
