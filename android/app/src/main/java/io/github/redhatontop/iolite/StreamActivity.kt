package io.github.redhatontop.iolite

import android.animation.ValueAnimator
import android.app.Activity
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import android.view.KeyEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.TextView

class StreamActivity : Activity() {
    private lateinit var prefs: Prefs
    private lateinit var endpoint: Endpoint
    private lateinit var root: FrameLayout
    private lateinit var aspectFrame: AspectFrame
    private lateinit var surface: SurfaceView
    private lateinit var keyboard: KeyboardBridge
    private lateinit var touch: TouchInput
    private lateinit var statusOverlay: LinearLayout
    private lateinit var statusText: TextView
    private lateinit var statusLine: View
    private lateinit var pairingOverlay: LinearLayout
    private lateinit var pairTitle: TextView
    private lateinit var pairCode: TextView
    private lateinit var banner: LinearLayout
    private lateinit var bannerText: TextView
    private lateinit var toolbar: LinearLayout
    private lateinit var toolbarHandle: View
    private lateinit var inputModeButton: ImageButton
    private lateinit var extraKeys: LinearLayout
    private lateinit var ctrlKey: TextView
    private lateinit var altKey: TextView
    private lateinit var superKey: TextView

    private val remote = RemoteSession()
    private val handler = Handler(Looper.getMainLooper())
    private val hideToolbar = Runnable { slideToolbar(false) }
    private val reconnect = Runnable { startSession(reconnecting = true) }
    private var statusAnimator: ValueAnimator? = null

    @Volatile
    private var session: Session? = null

    @Volatile
    private var decoder: VideoDecoder? = null

    @Volatile
    private var audio: AudioPlayer? = null

    private var frameSize: Pair<Int, Int>? = null
    private var started = false
    private var surfaceReady = false
    private var welcomed = false
    private var attempt = 0
    private var toolbarShown = false
    private var imeShown = false

    private val surfaceCallback = object : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) {
            surfaceReady = true
            if (session != null && welcomed) {
                startDecoder()
                Native.requestKeyframe(remote.handle)
            } else {
                startSession()
            }
        }

        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) = Unit

        override fun surfaceDestroyed(holder: SurfaceHolder) {
            surfaceReady = false
            releaseDecoder()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_stream)
        endpoint = Endpoint(
            intent.getStringExtra(EXTRA_HOST).orEmpty(),
            intent.getIntExtra(EXTRA_PORT, Prefs.DEFAULT_PORT),
        )
        prefs = Prefs(this)
        bindViews()
        enterImmersive()
        surface.holder.addCallback(surfaceCallback)
        keyboard.remote = remote
        keyboard.onStickyChanged = ::refreshModifiers
        touch = TouchInput(surface, remote, prefs.inputMode, ::onTouchInteract)
        touch.attach()
        bindControls()
        renderInputMode()
        refreshModifiers()
        toolbar.post { toolbar.translationX = hiddenToolbarOffset() }
    }

    override fun onStart() {
        super.onStart()
        started = true
        startSession()
    }

    override fun onStop() {
        started = false
        handler.removeCallbacks(reconnect)
        closeSession()
        super.onStop()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        statusAnimator?.cancel()
        super.onDestroy()
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        when (event.keyCode) {
            KeyEvent.KEYCODE_BACK -> {
                if (event.action == KeyEvent.ACTION_UP) finish()
                return true
            }
            KeyEvent.KEYCODE_VOLUME_UP, KeyEvent.KEYCODE_VOLUME_DOWN, KeyEvent.KEYCODE_VOLUME_MUTE ->
                return super.dispatchKeyEvent(event)
        }
        val keysym = Keysyms.forKeyEvent(event) ?: return super.dispatchKeyEvent(event)
        when (event.action) {
            KeyEvent.ACTION_DOWN -> if (event.repeatCount == 0) keyboard.key(keysym, true)
            KeyEvent.ACTION_UP -> keyboard.key(keysym, false)
        }
        return true
    }

    private fun bindViews() {
        root = findViewById(R.id.streamRoot)
        aspectFrame = findViewById(R.id.aspectFrame)
        surface = findViewById(R.id.surface)
        keyboard = findViewById(R.id.keyboardBridge)
        statusOverlay = findViewById(R.id.statusOverlay)
        statusText = findViewById(R.id.statusText)
        statusLine = findViewById(R.id.statusLine)
        pairingOverlay = findViewById(R.id.pairingOverlay)
        pairTitle = findViewById(R.id.pairTitle)
        pairCode = findViewById(R.id.pairCode)
        banner = findViewById(R.id.banner)
        bannerText = findViewById(R.id.bannerText)
        toolbar = findViewById(R.id.toolbar)
        toolbarHandle = findViewById(R.id.edgeHandle)
        inputModeButton = findViewById(R.id.inputModeButton)
        extraKeys = findViewById(R.id.extraKeys)
        ctrlKey = findViewById(R.id.keyCtrl)
        altKey = findViewById(R.id.keyAlt)
        superKey = findViewById(R.id.keySuper)
    }

    private fun bindControls() {
        root.setOnApplyWindowInsetsListener { _, insets ->
            imeShown = insets.isVisible(WindowInsets.Type.ime())
            extraKeys.visibility = if (imeShown) View.VISIBLE else View.GONE
            insets
        }
        toolbarHandle.setOnClickListener { slideToolbar(!toolbarShown) }
        findViewById<View>(R.id.keyboardButton).setOnClickListener {
            if (imeShown) keyboard.hide() else keyboard.show()
            slideToolbar(false)
        }
        inputModeButton.setOnClickListener {
            toggleInputMode()
            slideToolbar(true)
        }
        findViewById<View>(R.id.disconnectButton).setOnClickListener { disconnect() }
        findViewById<View>(R.id.pairCancelButton).setOnClickListener { cancelPairing() }
        findViewById<View>(R.id.reconnectButton).setOnClickListener {
            hideBanner()
            startSession()
        }
        findViewById<View>(R.id.backButton).setOnClickListener { finish() }
        bindKey(R.id.keyEsc, Keysyms.ESCAPE)
        bindKey(R.id.keyTab, Keysyms.TAB)
        bindKey(R.id.keyLeft, Keysyms.DPAD_LEFT)
        bindKey(R.id.keyUp, Keysyms.DPAD_UP)
        bindKey(R.id.keyDown, Keysyms.DPAD_DOWN)
        bindKey(R.id.keyRight, Keysyms.DPAD_RIGHT)
        ctrlKey.setOnClickListener { keyboard.toggleSticky(Keysyms.CTRL_LEFT) }
        altKey.setOnClickListener { keyboard.toggleSticky(Keysyms.ALT_LEFT) }
        superKey.setOnClickListener { keyboard.toggleSticky(Keysyms.META_LEFT) }
    }

    private fun bindKey(id: Int, keysym: Int) {
        findViewById<View>(id).setOnClickListener { keyboard.send(keysym) }
    }

    private fun refreshModifiers() {
        styleModifier(ctrlKey, Keysyms.CTRL_LEFT)
        styleModifier(altKey, Keysyms.ALT_LEFT)
        styleModifier(superKey, Keysyms.META_LEFT)
    }

    private fun styleModifier(key: TextView, keysym: Int) {
        val active = keyboard.isSticky(keysym)
        key.setTextColor(getColor(if (active) R.color.honey else R.color.text))
        key.setBackgroundResource(if (active) R.drawable.bg_key_active else R.drawable.bg_key)
    }

    private fun toggleInputMode() {
        touch.mode = if (touch.mode == InputMode.TOUCH) InputMode.TRACKPAD else InputMode.TOUCH
        prefs.inputMode = touch.mode
        renderInputMode()
    }

    private fun renderInputMode() {
        val trackpad = touch.mode == InputMode.TRACKPAD
        inputModeButton.setImageResource(if (trackpad) R.drawable.ic_cursor else R.drawable.ic_hand)
        inputModeButton.contentDescription = getString(if (trackpad) R.string.input_trackpad else R.string.input_touch)
    }

    private fun onTouchInteract() {
        if (toolbarShown) slideToolbar(false)
    }

    private fun slideToolbar(shown: Boolean) {
        toolbarShown = shown
        handler.removeCallbacks(hideToolbar)
        if (shown) handler.postDelayed(hideToolbar, TOOLBAR_IDLE_MS)
        toolbar.animate()
            .translationX(if (shown) 0f else hiddenToolbarOffset())
            .setDuration(TOOLBAR_SLIDE_MS)
            .start()
    }

    private fun hiddenToolbarOffset(): Float = toolbar.width.toFloat() + dp(8f)

    private fun enterImmersive() {
        window.setDecorFitsSystemWindows(false)
        window.insetsController?.let {
            it.hide(WindowInsets.Type.systemBars())
            it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.attributes = window.attributes.apply {
            layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
    }

    private fun startSession(reconnecting: Boolean = false) {
        if (!started || !surfaceReady || session != null) return
        welcomed = false
        hideBanner()
        showStatus(
            if (reconnecting) getString(R.string.reconnecting) else getString(R.string.connecting_to, endpoint.host),
        )
        val newSession = Session()
        session = newSession
        val bounds = windowManager.currentWindowMetrics.bounds
        try {
            remote.handle = Native.connect(
                endpoint.host,
                endpoint.port,
                filesDir.absolutePath,
                prefs.pin(endpoint),
                deviceName(),
                bounds.width(),
                bounds.height(),
                true,
                newSession,
            )
        } catch (e: IllegalStateException) {
            session = null
            showBanner(e.message.orEmpty())
        }
    }

    private fun closeSession() {
        val handle = remote.handle
        session = null
        remote.handle = 0L
        welcomed = false
        releaseMedia()
        Native.close(handle)
    }

    private fun handleWelcome(fingerprint: String, width: Int, height: Int, opusHead: ByteArray?) {
        welcomed = true
        attempt = 0
        prefs.setPin(endpoint, fingerprint)
        prefs.remember(endpoint)
        hideOverlays()
        aspectFrame.setAspect(width, height)
        remote.frameWidth = width
        remote.frameHeight = height
        frameSize = width to height
        releaseAudio()
        audio = opusHead?.let { head -> runCatching { AudioPlayer(head) }.getOrNull() }
        startDecoder()
        Native.requestKeyframe(remote.handle)
    }

    private fun handleClosed(reason: String) {
        session = null
        remote.handle = 0L
        releaseMedia()
        if (welcomed && started && !reason.contains(TAKEN_OVER)) {
            scheduleReconnect()
        } else {
            welcomed = false
            showBanner(reason)
        }
    }

    private fun scheduleReconnect() {
        val delaySeconds = RETRY_SECONDS.getOrElse(attempt) { RETRY_SECONDS.last() }
        attempt++
        showStatus(getString(R.string.reconnecting))
        handler.postDelayed(reconnect, delaySeconds * 1000L)
    }

    private fun startDecoder() {
        releaseDecoder()
        val (width, height) = frameSize ?: return
        if (!surfaceReady) return
        decoder = runCatching {
            VideoDecoder(
                surface.holder.surface,
                width,
                height,
                requestKeyframe = { Native.requestKeyframe(remote.handle) },
                onFailure = { runOnUiThread { rebuildDecoder() } },
            )
        }.onFailure { Log.e(TAG, "video decoder could not start", it) }.getOrNull()
        if (decoder == null) showBanner(getString(R.string.video_failed))
    }

    private fun rebuildDecoder() {
        if (session == null || !welcomed) return
        startDecoder()
        Native.requestKeyframe(remote.handle)
    }

    private fun releaseDecoder() {
        decoder?.release()
        decoder = null
    }

    private fun releaseAudio() {
        audio?.release()
        audio = null
    }

    private fun releaseMedia() {
        releaseDecoder()
        releaseAudio()
    }

    private fun disconnect() {
        closeSession()
        slideToolbar(false)
        showBanner(getString(R.string.disconnected))
    }

    private fun cancelPairing() {
        closeSession()
        showBanner(getString(R.string.pair_cancelled))
    }

    private fun showPairing(code: String) {
        hideStatus()
        pairTitle.text = getString(R.string.pair_with, endpoint.host)
        pairCode.text = code.chunked(3).joinToString(" ")
        pairingOverlay.visibility = View.VISIBLE
    }

    private fun showStatus(text: String) {
        statusText.text = text
        pairingOverlay.visibility = View.GONE
        statusOverlay.visibility = View.VISIBLE
        if (statusAnimator == null) startStatusAnimation()
    }

    private fun hideStatus() {
        statusOverlay.visibility = View.GONE
        statusAnimator?.cancel()
        statusAnimator = null
    }

    private fun startStatusAnimation() {
        statusAnimator = ValueAnimator.ofFloat(0f, 1f).apply {
            duration = STATUS_PULSE_MS
            repeatCount = ValueAnimator.INFINITE
            repeatMode = ValueAnimator.REVERSE
            addUpdateListener {
                val fraction = it.animatedFraction
                statusLine.alpha = 0.3f + 0.7f * fraction
                statusLine.scaleX = 0.4f + 0.6f * fraction
            }
            start()
        }
    }

    private fun hideOverlays() {
        hideStatus()
        pairingOverlay.visibility = View.GONE
    }

    private fun showBanner(reason: String) {
        hideOverlays()
        bannerText.text = reason
        banner.visibility = View.VISIBLE
    }

    private fun hideBanner() {
        banner.visibility = View.GONE
    }

    private fun deviceName(): String =
        Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL

    private fun dp(value: Float): Float = value * resources.displayMetrics.density

    private inner class Session : Native.Listener {
        override fun onPairingCode(code: String) {
            runOnUiThread {
                if (session === this@Session) showPairing(code)
            }
        }

        override fun onWelcome(hostFingerprint: String, hostName: String, width: Int, height: Int, opusHead: ByteArray?) {
            runOnUiThread {
                if (session === this@Session) handleWelcome(hostFingerprint, width, height, opusHead)
            }
        }

        override fun onVideo(data: ByteArray, ptsUs: Long, keyframe: Boolean) {
            if (session === this@Session) decoder?.submit(data, ptsUs, keyframe)
        }

        override fun onAudio(data: ByteArray, ptsUs: Long) {
            if (session === this@Session) audio?.submit(data, ptsUs)
        }

        override fun onClosed(reason: String) {
            runOnUiThread {
                if (session === this@Session) handleClosed(reason)
            }
        }
    }

    companion object {
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        private const val TAG = "iolite"
        private const val TOOLBAR_IDLE_MS = 4000L
        private const val TOOLBAR_SLIDE_MS = 180L
        private const val STATUS_PULSE_MS = 900L
        private val RETRY_SECONDS = longArrayOf(1, 2, 4, 8, 16, 30)

        /** Part of the host's close reason when another device takes the session; retrying would take it back. */
        private const val TAKEN_OVER = "took over"
    }
}
