package io.github.redhatontop.iolite

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

class MainActivity : Activity() {
    private lateinit var prefs: Prefs
    private lateinit var hostInput: EditText
    private lateinit var portInput: EditText
    private lateinit var recentList: LinearLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        prefs = Prefs(this)
        hostInput = findViewById(R.id.hostInput)
        portInput = findViewById(R.id.portInput)
        recentList = findViewById(R.id.recentList)
        findViewById<TextView>(R.id.fingerprintText).text = fingerprintLabel()
        findViewById<Button>(R.id.connectButton).setOnClickListener { connectFromInput() }

        val extraHost = intent.getStringExtra(StreamActivity.EXTRA_HOST)
        if (extraHost != null) {
            hostInput.setText(extraHost)
            portInput.setText(intent.getIntExtra(StreamActivity.EXTRA_PORT, Prefs.DEFAULT_PORT).toString())
            if (intent.getBooleanExtra(EXTRA_AUTOCONNECT, false)) connectFromInput()
        } else {
            prefs.lastHost?.let { hostInput.setText(it) }
            portInput.setText(prefs.lastPort.toString())
        }
    }

    override fun onResume() {
        super.onResume()
        renderRecent()
    }

    private fun connectFromInput() {
        val host = hostInput.text.toString().trim()
        val port = portInput.text.toString().toIntOrNull()?.takeIf { it in 1..65535 }
        hostInput.error = if (host.isEmpty()) getString(R.string.host_required) else null
        portInput.error = if (port == null) getString(R.string.port_invalid) else null
        if (host.isEmpty() || port == null) return
        connect(Endpoint(host, port))
    }

    private fun connect(endpoint: Endpoint) {
        prefs.remember(endpoint)
        startActivity(
            Intent(this, StreamActivity::class.java)
                .putExtra(StreamActivity.EXTRA_HOST, endpoint.host)
                .putExtra(StreamActivity.EXTRA_PORT, endpoint.port),
        )
    }

    private fun renderRecent() {
        recentList.removeAllViews()
        prefs.recent().forEach { recentList.addView(recentRow(it)) }
    }

    private fun recentRow(endpoint: Endpoint): View {
        val paired = prefs.pin(endpoint) != null
        val hostText = TextView(this).apply {
            text = endpoint.host
            setTextColor(getColor(R.color.text))
            textSize = 16f
        }
        val stateText = TextView(this).apply {
            setText(if (paired) R.string.paired else R.string.not_paired)
            setTextColor(getColor(if (paired) R.color.violet else R.color.text_dim))
            textSize = 12f
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(4f), dp(12f), dp(4f), dp(12f))
            setBackgroundResource(R.drawable.bg_key)
            addView(hostText)
            addView(stateText)
            setOnClickListener { connect(endpoint) }
            setOnLongClickListener {
                confirmForget(endpoint)
                true
            }
        }
    }

    private fun confirmForget(endpoint: Endpoint) {
        AlertDialog.Builder(this)
            .setTitle(getString(R.string.forget_title, endpoint.host))
            .setPositiveButton(R.string.forget) { _, _ ->
                prefs.forget(endpoint)
                renderRecent()
            }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private fun fingerprintLabel(): String {
        val fingerprint = Native.fingerprint(filesDir.absolutePath).take(16).chunked(4).joinToString(" ")
        return "${getString(R.string.this_device)}  $fingerprint"
    }

    private fun dp(value: Float): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        private const val EXTRA_AUTOCONNECT = "autoconnect"
    }
}
