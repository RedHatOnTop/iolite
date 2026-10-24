package io.github.redhatontop.iolite

/**
 * JNI surface of `libiolite_android.so` (crates/iolite-android).
 *
 * A handle names one session. Calls with a closed or unknown handle are ignored. Listener
 * methods run on a native thread, one at a time and in protocol order; `onClosed` is always
 * the last call for a handle.
 */
object Native {
    init {
        System.loadLibrary("iolite_android")
    }

    const val TOUCH_DOWN = 0
    const val TOUCH_MOVE = 1
    const val TOUCH_UP = 2
    const val TOUCH_CANCEL = 3

    const val BUTTON_LEFT = 0
    const val BUTTON_MIDDLE = 1
    const val BUTTON_RIGHT = 2

    interface Listener {
        /** Not paired yet: show [code] until the host user accepts or declines. */
        fun onPairingCode(code: String)

        /** Streaming starts. Save [hostFingerprint] and pass it as `pinnedHost` next time. */
        fun onWelcome(
            hostFingerprint: String,
            hostName: String,
            width: Int,
            height: Int,
            opusHead: ByteArray?,
        )

        /** One AV1 temporal unit (low-overhead OBU format). */
        fun onVideo(data: ByteArray, ptsUs: Long, keyframe: Boolean)

        /** One Opus packet. */
        fun onAudio(data: ByteArray, ptsUs: Long)

        fun onClosed(reason: String)
    }

    /** Starts a session in the background and returns its handle. Throws on bad arguments. */
    @JvmStatic
    external fun connect(
        host: String,
        port: Int,
        dataDir: String,
        pinnedHost: String?,
        deviceName: String,
        width: Int,
        height: Int,
        audio: Boolean,
        listener: Listener,
    ): Long

    /** SHA-256 fingerprint (hex) of this device's client certificate, created on first use. */
    @JvmStatic
    external fun fingerprint(dataDir: String): String

    /** Coordinates are normalized to the video frame, 0..1. */
    @JvmStatic
    external fun touch(handle: Long, phase: Int, slot: Int, x: Float, y: Float)

    @JvmStatic
    external fun pointerMove(handle: Long, x: Float, y: Float)

    /** Relative motion in video frame pixels. */
    @JvmStatic
    external fun pointerRelative(handle: Long, dx: Float, dy: Float)

    @JvmStatic
    external fun button(handle: Long, button: Int, down: Boolean)

    /** Smooth scroll in video frame pixels; positive dy moves toward the end, like a wheel turned down. */
    @JvmStatic
    external fun scroll(handle: Long, dx: Float, dy: Float, finish: Boolean)

    /** X11 keysym press or release. */
    @JvmStatic
    external fun key(handle: Long, keysym: Int, down: Boolean)

    @JvmStatic
    external fun text(handle: Long, text: String)

    @JvmStatic
    external fun requestKeyframe(handle: Long)

    @JvmStatic
    external fun close(handle: Long)
}
