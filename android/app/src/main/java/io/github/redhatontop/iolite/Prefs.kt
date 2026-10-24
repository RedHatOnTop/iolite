package io.github.redhatontop.iolite

import android.content.Context

data class Endpoint(val host: String, val port: Int) {
    val key: String
        get() = "$host:$port"
}

class Prefs(context: Context) {
    private val prefs = context.getSharedPreferences(NAME, Context.MODE_PRIVATE)

    var lastHost: String?
        get() = prefs.getString(KEY_LAST_HOST, null)
        set(value) = prefs.edit().putString(KEY_LAST_HOST, value).apply()

    var lastPort: Int
        get() = prefs.getInt(KEY_LAST_PORT, DEFAULT_PORT)
        set(value) = prefs.edit().putInt(KEY_LAST_PORT, value).apply()

    var inputMode: InputMode
        get() = if (prefs.getString(KEY_INPUT_MODE, null) == "trackpad") InputMode.TRACKPAD else InputMode.TOUCH
        set(value) = prefs.edit().putString(KEY_INPUT_MODE, value.name.lowercase()).apply()

    fun recent(): List<Endpoint> =
        prefs.getString(KEY_RECENT, "").orEmpty().lineSequence().mapNotNull(::parseEndpoint).toList()

    fun remember(endpoint: Endpoint) {
        lastHost = endpoint.host
        lastPort = endpoint.port
        writeRecent((listOf(endpoint) + recent().filter { it != endpoint }).take(MAX_RECENT))
    }

    fun forget(endpoint: Endpoint) {
        writeRecent(recent().filter { it != endpoint })
        prefs.edit().remove(pinKey(endpoint)).apply()
    }

    fun pin(endpoint: Endpoint): String? = prefs.getString(pinKey(endpoint), null)

    fun setPin(endpoint: Endpoint, fingerprint: String) {
        prefs.edit().putString(pinKey(endpoint), fingerprint).apply()
    }

    private fun writeRecent(endpoints: List<Endpoint>) {
        prefs.edit().putString(KEY_RECENT, endpoints.joinToString("\n") { it.key }).apply()
    }

    private fun pinKey(endpoint: Endpoint) = "pin.${endpoint.key}"

    private fun parseEndpoint(line: String): Endpoint? {
        val host = line.substringBeforeLast(':', "")
        val port = line.substringAfterLast(':', "").toIntOrNull() ?: return null
        return if (host.isEmpty()) null else Endpoint(host, port)
    }

    companion object {
        const val DEFAULT_PORT = 46100
        private const val NAME = "iolite"
        private const val MAX_RECENT = 8
        private const val KEY_LAST_HOST = "lastHost"
        private const val KEY_LAST_PORT = "lastPort"
        private const val KEY_RECENT = "recent"
        private const val KEY_INPUT_MODE = "inputMode"
    }
}
