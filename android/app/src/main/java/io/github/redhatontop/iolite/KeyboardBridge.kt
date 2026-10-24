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

class KeyboardBridge @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs) {

    lateinit var remote: RemoteSession
    var onStickyChanged: (() -> Unit)? = null

    private val sticky = mutableSetOf<Int>()
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

    fun toggleSticky(keysym: Int) {
        if (!sticky.remove(keysym)) sticky.add(keysym)
        onStickyChanged?.invoke()
    }

    fun isSticky(keysym: Int): Boolean = keysym in sticky

    fun send(keysym: Int) {
        val modifiers = sticky.toList()
        modifiers.forEach { key(it, true) }
        press(keysym)
        modifiers.asReversed().forEach { key(it, false) }
        if (sticky.isNotEmpty()) {
            sticky.clear()
            onStickyChanged?.invoke()
        }
    }

    fun press(keysym: Int) {
        key(keysym, true)
        key(keysym, false)
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
        if (sticky.isEmpty() || composing.isNotEmpty()) return null
        return text.codePoints().toArray().singleOrNull()?.let(Keysyms::forShortcut)
    }

    private fun restartInput() {
        context.getSystemService(InputMethodManager::class.java).restartInput(this)
    }

    private inner class Connection(view: View) : BaseInputConnection(view, false) {
        override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
            val keysym = shortcut(text)
            if (keysym != null) send(keysym) else commit(text.toString())
            return true
        }

        override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
            val keysym = shortcut(text)
            if (keysym != null) {
                send(keysym)
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
            repeat(beforeLength.coerceIn(0, MAX_DELETE)) { press(Keysyms.DEL) }
            repeat(afterLength.coerceIn(0, MAX_DELETE)) { press(Keysyms.FORWARD_DEL) }
            return true
        }

        override fun sendKeyEvent(event: KeyEvent): Boolean {
            Keysyms.forKeyEvent(event)?.let { key(it, event.action == KeyEvent.ACTION_DOWN) }
            return true
        }
    }

    private companion object {
        const val MAX_DELETE = 64
    }
}
