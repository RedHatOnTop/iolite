package io.github.redhatontop.iolite

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

data class Endpoint(val host: String, val port: Int) {
    val key: String
        get() = "$host:$port"
}

/** A saved connection with its pairing and its own settings. */
data class Computer(
    val id: String,
    val endpoint: Endpoint,
    /** Chosen by the user; empty shows the name the computer reports. */
    val name: String = "",
    val hostName: String = "",
    /** Fingerprint of the computer's certificate, saved when it is first reached. */
    val pin: String? = null,
    val lastUsed: Long = 0L,
    val inputMode: InputMode = InputMode.TOUCH,
    val audio: Boolean = true,
) {
    val title: String
        get() = name.ifEmpty { hostName.ifEmpty { endpoint.host } }
}

class Prefs(context: Context) {
    private val prefs = context.getSharedPreferences(NAME, Context.MODE_PRIVATE)

    init {
        migrate()
    }

    var keyBarShown: Boolean
        get() = prefs.getBoolean(KEY_BAR_SHOWN, true)
        set(value) = prefs.edit().putBoolean(KEY_BAR_SHOWN, value).apply()

    var keyBarExpanded: Boolean
        get() = prefs.getBoolean(KEY_BAR_EXPANDED, true)
        set(value) = prefs.edit().putBoolean(KEY_BAR_EXPANDED, value).apply()

    var keyBarPage: Int
        get() = prefs.getInt(KEY_BAR_PAGE, 0)
        set(value) = prefs.edit().putInt(KEY_BAR_PAGE, value).apply()

    /** Where the floating button rests: on the end edge or the start edge, and how far down. */
    var pillAtEnd: Boolean
        get() = prefs.getBoolean(KEY_PILL_END, true)
        set(value) = prefs.edit().putBoolean(KEY_PILL_END, value).apply()

    var pillY: Float
        get() = prefs.getFloat(KEY_PILL_Y, 0.85f)
        set(value) = prefs.edit().putFloat(KEY_PILL_Y, value).apply()

    /** Most recently used first; computers never reached keep the order they were added in. */
    fun computers(): List<Computer> = readComputers().sortedByDescending { it.lastUsed }

    fun computer(id: String): Computer? = readComputers().firstOrNull { it.id == id }

    /** The saved computer at [endpoint], or a new one saved for it. */
    fun computerFor(endpoint: Endpoint): Computer =
        readComputers().firstOrNull { it.endpoint == endpoint }
            ?: Computer(UUID.randomUUID().toString(), endpoint).also(::save)

    fun save(computer: Computer) {
        val computers = readComputers()
        val index = computers.indexOfFirst { it.id == computer.id }
        writeComputers(if (index < 0) computers + computer else computers.toMutableList().also { it[index] = computer })
    }

    /** Applies [change] to the saved copy, so edits made elsewhere are kept. */
    fun update(id: String, change: (Computer) -> Computer): Computer? =
        computer(id)?.let(change)?.also(::save)

    fun delete(id: String) {
        writeComputers(readComputers().filter { it.id != id })
    }

    fun shortcuts(): List<Shortcut> {
        val array = JSONArray(prefs.getString(KEY_SHORTCUTS, "[]"))
        return (0 until array.length()).map { i ->
            val o = array.getJSONObject(i)
            val keys = o.getJSONArray("keys")
            Shortcut((0 until keys.length()).map(keys::getInt), o.optString("name"))
        }
    }

    fun saveShortcuts(shortcuts: List<Shortcut>) {
        val array = JSONArray()
        shortcuts.forEach { s ->
            array.put(JSONObject().put("name", s.name).put("keys", JSONArray(s.keys)))
        }
        prefs.edit().putString(KEY_SHORTCUTS, array.toString()).apply()
    }

    private fun readComputers(): List<Computer> {
        val array = JSONArray(prefs.getString(KEY_COMPUTERS, "[]"))
        return (0 until array.length()).map { i ->
            val o = array.getJSONObject(i)
            Computer(
                id = o.getString("id"),
                endpoint = Endpoint(o.getString("host"), o.getInt("port")),
                name = o.optString("name"),
                hostName = o.optString("hostName"),
                pin = if (o.has("pin")) o.getString("pin") else null,
                lastUsed = o.optLong("lastUsed"),
                inputMode = if (o.optString("inputMode") == "trackpad") InputMode.TRACKPAD else InputMode.TOUCH,
                audio = o.optBoolean("audio", true),
            )
        }
    }

    private fun writeComputers(computers: List<Computer>) {
        val array = JSONArray()
        computers.forEach { c ->
            val o = JSONObject()
                .put("id", c.id)
                .put("host", c.endpoint.host)
                .put("port", c.endpoint.port)
                .put("name", c.name)
                .put("hostName", c.hostName)
                .put("lastUsed", c.lastUsed)
                .put("inputMode", c.inputMode.name.lowercase())
                .put("audio", c.audio)
            c.pin?.let { o.put("pin", it) }
            array.put(o)
        }
        prefs.edit().putString(KEY_COMPUTERS, array.toString()).apply()
    }

    /** Turns the recent list and the pins of the first version into saved computers. */
    private fun migrate() {
        if (prefs.contains(KEY_COMPUTERS)) return
        val mode = if (prefs.getString(LEGACY_INPUT_MODE, null) == "trackpad") InputMode.TRACKPAD else InputMode.TOUCH
        val now = System.currentTimeMillis()
        val endpoints = prefs.getString(LEGACY_RECENT, "").orEmpty().lineSequence().mapNotNull(::parseEndpoint).toList()
        val computers = endpoints.mapIndexed { i, endpoint ->
            val pin = prefs.getString("pin.${endpoint.key}", null)
            Computer(
                UUID.randomUUID().toString(),
                endpoint,
                pin = pin,
                lastUsed = if (pin != null) now - i else 0L,
                inputMode = mode,
            )
        }
        writeComputers(computers)
        prefs.edit().apply {
            endpoints.forEach { remove("pin.${it.key}") }
            listOf(LEGACY_RECENT, LEGACY_INPUT_MODE, LEGACY_LAST_HOST, LEGACY_LAST_PORT).forEach { remove(it) }
        }.apply()
    }

    private fun parseEndpoint(line: String): Endpoint? {
        val host = line.substringBeforeLast(':', "")
        val port = line.substringAfterLast(':', "").toIntOrNull() ?: return null
        return if (host.isEmpty()) null else Endpoint(host, port)
    }

    companion object {
        const val DEFAULT_PORT = 46100
        private const val NAME = "iolite"
        private const val KEY_COMPUTERS = "computers"
        private const val KEY_SHORTCUTS = "shortcuts"
        private const val KEY_BAR_SHOWN = "keyBarShown"
        private const val KEY_BAR_EXPANDED = "keyBarExpanded"
        private const val KEY_BAR_PAGE = "keyBarPage"
        private const val KEY_PILL_END = "pillAtEnd"
        private const val KEY_PILL_Y = "pillY"
        private const val LEGACY_RECENT = "recent"
        private const val LEGACY_INPUT_MODE = "inputMode"
        private const val LEGACY_LAST_HOST = "lastHost"
        private const val LEGACY_LAST_PORT = "lastPort"
    }
}
