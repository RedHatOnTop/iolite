package io.github.redhatontop.iolite

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.text.format.DateUtils
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView

class MainActivity : Activity() {
    private lateinit var prefs: Prefs
    private lateinit var hostInput: EditText
    private lateinit var portInput: EditText
    private lateinit var computersLabel: View
    private lateinit var computerList: LinearLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        prefs = Prefs(this)
        hostInput = findViewById(R.id.hostInput)
        portInput = findViewById(R.id.portInput)
        computersLabel = findViewById(R.id.computersLabel)
        computerList = findViewById(R.id.computerList)
        computerList.clipToOutline = true
        findViewById<TextView>(R.id.fingerprintText).text = fingerprintLabel()
        findViewById<Button>(R.id.connectButton).setOnClickListener { connectFromInput() }

        val extraHost = intent.getStringExtra(StreamActivity.EXTRA_HOST)
        if (extraHost != null) {
            hostInput.setText(extraHost)
            portInput.setText(intent.getIntExtra(StreamActivity.EXTRA_PORT, Prefs.DEFAULT_PORT).toString())
            if (savedInstanceState == null && intent.getBooleanExtra(EXTRA_AUTOCONNECT, false)) connectFromInput()
        }
    }

    override fun onResume() {
        super.onResume()
        renderComputers()
    }

    private fun connectFromInput() {
        val endpoint = readEndpoint(hostInput, portInput) ?: return
        connect(prefs.computerFor(endpoint))
    }

    private fun connect(computer: Computer) {
        startActivity(
            Intent(this, StreamActivity::class.java).putExtra(StreamActivity.EXTRA_COMPUTER, computer.id),
        )
    }

    private fun renderComputers() {
        val computers = prefs.computers()
        computerList.removeAllViews()
        computers.forEachIndexed { index, computer ->
            if (index > 0) {
                computerList.addView(
                    View(this).apply { setBackgroundResource(R.color.hairline) },
                    LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(1f)).apply {
                        marginStart = dp(54f)
                    },
                )
            }
            computerList.addView(computerRow(computer))
        }
        val visibility = if (computers.isEmpty()) View.GONE else View.VISIBLE
        computersLabel.visibility = visibility
        computerList.visibility = visibility
    }

    private fun computerRow(computer: Computer): View {
        val row = layoutInflater.inflate(R.layout.item_computer, computerList, false)
        val paired = computer.pin != null
        row.findViewById<ImageView>(R.id.mark)
            .setImageResource(if (paired) R.drawable.ic_facet else R.drawable.ic_facet_outline)
        row.findViewById<TextView>(R.id.computerName).text = computer.title
        row.findViewById<TextView>(R.id.computerDetail).text = buildString {
            append(computer.endpoint.host)
            if (computer.endpoint.port != Prefs.DEFAULT_PORT) append(':').append(computer.endpoint.port)
            if (!paired) append(" · ").append(getString(R.string.not_paired))
        }
        row.findViewById<TextView>(R.id.computerLastUsed).text = lastUsedLabel(computer.lastUsed)
        row.findViewById<View>(R.id.computerEdit).setOnClickListener { edit(computer) }
        row.setOnClickListener { connect(computer) }
        row.setOnLongClickListener {
            edit(computer)
            true
        }
        return row
    }

    private fun edit(computer: Computer) {
        ComputerDialog(this, prefs, computer, ::renderComputers).show()
    }

    private fun lastUsedLabel(lastUsed: Long): CharSequence {
        if (lastUsed == 0L) return getString(R.string.never_connected)
        val now = System.currentTimeMillis()
        if (now - lastUsed < DateUtils.MINUTE_IN_MILLIS) return getString(R.string.just_now)
        return DateUtils.getRelativeTimeSpanString(
            lastUsed,
            now,
            DateUtils.MINUTE_IN_MILLIS,
            DateUtils.FORMAT_ABBREV_RELATIVE,
        )
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

/** The host and port typed into a pair of fields, or null after flagging what is wrong. */
fun readEndpoint(hostInput: EditText, portInput: EditText): Endpoint? {
    val context = hostInput.context
    val host = hostInput.text.toString().trim()
    val port = portInput.text.toString().toIntOrNull()?.takeIf { it in 1..65535 }
    hostInput.error = if (host.isEmpty()) context.getString(R.string.host_required) else null
    portInput.error = if (port == null) context.getString(R.string.port_invalid) else null
    if (host.isEmpty() || port == null) return null
    return Endpoint(host, port)
}
