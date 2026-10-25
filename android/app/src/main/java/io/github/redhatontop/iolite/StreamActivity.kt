package io.github.redhatontop.iolite

import android.animation.ValueAnimator
import android.app.Activity
import android.content.res.ColorStateList
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
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.PopupWindow
import android.widget.TextView

class StreamActivity : Activity() {
    private lateinit var prefs: Prefs
    private lateinit var computer: Computer
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
    private lateinit var keyBar: KeyBar
    private lateinit var pill: FloatingPill

    private val remote = RemoteSession()
    private val handler = Handler(Looper.getMainLooper())
    private val reconnect = Runnable { startSession(reconnecting = true) }
    private val resize = Runnable { sendResize() }
    private var statusAnimator: ValueAnimator? = null

    @Volatile
    private var session: Session? = null

    @Volatile
    private var decoder: VideoDecoder? = null

    @Volatile
    private var audio: AudioPlayer? = null

    private val decoderLock = Any()

    @Volatile
    private var frameSize: Pair<Int, Int>? = null
    private var started = false

    @Volatile
    private var surfaceReady = false
    private var welcomed = false
    private var attempt = 0
    private var imeShown = false
    private var imeInset = 0

    /** The video area the computer was last told about. */
    private var sentArea: Pair<Int, Int>? = null

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
        prefs = Prefs(this)
        computer = intent.getStringExtra(EXTRA_COMPUTER)?.let(prefs::computer) ?: run {
            finish()
            return
        }
        bindViews()
        enterImmersive()
        surface.holder.addCallback(surfaceCallback)
        keyboard.remote = remote
        keyboard.onLatchChanged = { keyBar.refreshModifiers() }
        touch = TouchInput(surface, remote, computer.inputMode, keyboard, ::onTouchInteract, ::toggleKeyboard)
        touch.attach()
        bindControls()
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
            KeyEvent.ACTION_DOWN -> if (event.repeatCount == 0) {
                keyboard.finishComposition()
                keyboard.key(keysym, true)
            }
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
        keyBar = findViewById(R.id.keyBar)
        pill = FloatingPill(findViewById(R.id.pill), prefs) { setKeyBarShown(true) }
    }

    private fun bindControls() {
        // The window draws behind the input method, so the bar and the video make room for it here.
        root.setOnApplyWindowInsetsListener { _, insets ->
            imeShown = insets.isVisible(WindowInsets.Type.ime())
            imeInset = insets.getInsets(WindowInsets.Type.ime()).bottom
            val cutout = insets.getInsets(WindowInsets.Type.displayCutout())
            keyBar.setPadding(cutout.left, 0, cutout.right, 0)
            keyBar.setKeyboardShown(imeShown)
            layoutControls()
            insets
        }
        keyBar.addOnLayoutChangeListener { _, _, top, _, bottom, _, oldTop, _, oldBottom ->
            if (bottom - top != oldBottom - oldTop) root.post(::layoutControls)
        }
        root.addOnLayoutChangeListener { _, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom ->
            if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) areaChanged()
        }
        keyBar.keyboard = keyboard
        keyBar.onKeyboard = ::toggleKeyboard
        keyBar.onMore = ::showMore
        keyBar.onHide = { setKeyBarShown(false) }
        keyBar.onAddShortcut = ::addShortcut
        keyBar.onEditShortcut = ::editShortcut
        keyBar.onPageChanged = {
            prefs.keyBarPage = keyBar.page.ordinal
            prefs.keyBarExpanded = keyBar.expanded
        }
        keyBar.restore(KeyBar.Page.entries.getOrElse(prefs.keyBarPage) { KeyBar.Page.KEYS }, prefs.keyBarExpanded)
        keyBar.setShortcuts(prefs.shortcuts())
        keyBar.refreshModifiers()
        setKeyBarShown(prefs.keyBarShown)
        findViewById<View>(R.id.pairCancelButton).setOnClickListener { cancelPairing() }
        findViewById<View>(R.id.reconnectButton).setOnClickListener {
            hideBanner()
            startSession()
        }
        findViewById<View>(R.id.backButton).setOnClickListener { finish() }
    }

    private fun layoutControls() {
        val params = keyBar.layoutParams as FrameLayout.LayoutParams
        if (params.bottomMargin != imeInset) {
            params.bottomMargin = imeInset
            keyBar.layoutParams = params
        }
        val bar = if (keyBar.visibility == View.VISIBLE) keyBar.height else 0
        aspectFrame.setPadding(0, 0, 0, imeInset + bar)
        pill.setBottomInset(imeInset)
        areaChanged()
    }

    /**
     * The area the video may fill: the window less the key bar. The input method is left out,
     * so opening it does not resize the computer's screen.
     */
    private fun videoArea(): Pair<Int, Int> {
        val bar = if (keyBar.visibility == View.VISIBLE) keyBar.height else 0
        if (root.width > 0 && root.height > bar) return root.width to root.height - bar
        val bounds = windowManager.currentWindowMetrics.bounds
        return bounds.width() to bounds.height()
    }

    /** A fold, a turn or the key bar changed the video area; a screen made for this device follows it. */
    private fun areaChanged() {
        if (!welcomed || computer.display == DisplayMode.MIRROR) return
        handler.removeCallbacks(resize)
        handler.postDelayed(resize, RESIZE_SETTLE_MS)
    }

    private fun sendResize() {
        if (!welcomed || session == null) return
        val area = videoArea()
        if (area == sentArea) return
        sentArea = area
        Native.resize(remote.handle, area.first, area.second)
    }

    /** GNOME scale for a screen made for this device: a little under Android's, since a phone is held closer than a laptop's screen. */
    private fun displayScale(): Float = resources.displayMetrics.density * SCALE_PER_DENSITY

    private fun setKeyBarShown(shown: Boolean) {
        prefs.keyBarShown = shown
        keyBar.visibility = if (shown) View.VISIBLE else View.GONE
        findViewById<View>(R.id.pill).visibility = if (shown) View.GONE else View.VISIBLE
        layoutControls()
    }

    private fun toggleKeyboard() {
        if (imeShown) keyboard.hide() else keyboard.show()
    }

    private fun showMore(anchor: View) {
        val content = layoutInflater.inflate(R.layout.popup_more, root, false)
        val width = dp(MENU_WIDTH_DP).toInt()
        val popup = PopupWindow(content, width, ViewGroup.LayoutParams.WRAP_CONTENT, true)
        val touchItem = content.findViewById<TextView>(R.id.menuTouch)
        val trackpadItem = content.findViewById<TextView>(R.id.menuTrackpad)
        styleMenuItem(touchItem, touch.mode == InputMode.TOUCH)
        styleMenuItem(trackpadItem, touch.mode == InputMode.TRACKPAD)
        mapOf(
            DisplayMode.OWN to R.id.menuScreenOwn,
            DisplayMode.EXTEND to R.id.menuScreenExtend,
            DisplayMode.MIRROR to R.id.menuScreenMirror,
        ).forEach { (mode, id) ->
            val item = content.findViewById<TextView>(id)
            styleMenuItem(item, computer.display == mode)
            item.setOnClickListener {
                popup.dismiss()
                setDisplay(mode)
            }
        }
        touchItem.setOnClickListener {
            setInputMode(InputMode.TOUCH)
            popup.dismiss()
        }
        trackpadItem.setOnClickListener {
            setInputMode(InputMode.TRACKPAD)
            popup.dismiss()
        }
        content.findViewById<View>(R.id.menuDisconnect).setOnClickListener {
            popup.dismiss()
            disconnect()
        }
        popup.elevation = dp(8f)
        content.measure(
            View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.UNSPECIFIED,
        )
        popup.showAsDropDown(
            anchor,
            anchor.width - width,
            -anchor.height - content.measuredHeight - dp(6f).toInt(),
        )
    }

    private fun styleMenuItem(item: TextView, active: Boolean) {
        val color = getColor(if (active) R.color.honey else R.color.text)
        item.setTextColor(color)
        item.compoundDrawableTintList = ColorStateList.valueOf(color)
    }

    private fun setInputMode(mode: InputMode) {
        touch.mode = mode
        computer = prefs.update(computer.id) { it.copy(inputMode = mode) } ?: computer
    }

    /** The computer makes the screen when a session starts, so a new mode takes a new session. */
    private fun setDisplay(mode: DisplayMode) {
        if (mode == computer.display) return
        computer = prefs.update(computer.id) { it.copy(display = mode) } ?: computer.copy(display = mode)
        handler.removeCallbacks(reconnect)
        closeSession()
        startSession()
    }

    private fun addShortcut() {
        ShortcutDialog(this, null, onSave = { saveShortcuts(prefs.shortcuts() + it) }, onDelete = {}).show()
    }

    private fun editShortcut(index: Int) {
        val shortcuts = prefs.shortcuts()
        val existing = shortcuts.getOrNull(index) ?: return
        ShortcutDialog(
            this,
            existing,
            onSave = { edited -> saveShortcuts(shortcuts.toMutableList().also { it[index] = edited }) },
            onDelete = { saveShortcuts(shortcuts.filterIndexed { i, _ -> i != index }) },
        ).show()
    }

    private fun saveShortcuts(shortcuts: List<Shortcut>) {
        prefs.saveShortcuts(shortcuts)
        keyBar.setShortcuts(shortcuts)
    }

    private fun onTouchInteract() {
        keyboard.finishComposition()
    }

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
        computer = prefs.computer(computer.id) ?: computer
        welcomed = false
        hideBanner()
        showStatus(
            if (reconnecting) getString(R.string.reconnecting) else getString(R.string.connecting_to, computer.title),
        )
        val newSession = Session()
        session = newSession
        val area = videoArea()
        sentArea = area
        try {
            remote.handle = Native.connect(
                computer.endpoint.host,
                computer.endpoint.port,
                filesDir.absolutePath,
                computer.pin,
                deviceName(),
                area.first,
                area.second,
                computer.audio,
                computer.display.wire,
                displayScale(),
                newSession,
            )
        } catch (e: IllegalStateException) {
            session = null
            showBanner(e.message.orEmpty())
        }
    }

    private fun closeSession() {
        keyboard.resetLatches()
        val handle = remote.handle
        session = null
        remote.handle = 0L
        welcomed = false
        releaseMedia()
        Native.close(handle)
    }

    private fun handleWelcome(fingerprint: String, hostName: String, width: Int, height: Int, opusHead: ByteArray?) {
        welcomed = true
        attempt = 0
        computer = prefs.update(computer.id) {
            it.copy(pin = fingerprint, hostName = hostName, lastUsed = System.currentTimeMillis())
        } ?: computer
        hideOverlays()
        aspectFrame.setAspect(width, height)
        releaseAudio()
        audio = opusHead?.let { head -> runCatching { AudioPlayer(head) }.getOrNull() }
        if (decoder == null) {
            startDecoder()
            Native.requestKeyframe(remote.handle)
        }
        areaChanged()
    }

    private fun handleClosed(reason: String) {
        session = null
        remote.handle = 0L
        keyboard.resetLatches()
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

    /** Runs on the UI thread or, before the first frame of a new size, on the native thread. */
    private fun startDecoder() {
        synchronized(decoderLock) {
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
            if (decoder == null) runOnUiThread { showBanner(getString(R.string.video_failed)) }
        }
    }

    private fun rebuildDecoder() {
        if (session == null || !welcomed) return
        startDecoder()
        Native.requestKeyframe(remote.handle)
    }

    private fun releaseDecoder() {
        synchronized(decoderLock) {
            decoder?.release()
            decoder = null
        }
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
        showBanner(getString(R.string.disconnected))
    }

    private fun cancelPairing() {
        closeSession()
        showBanner(getString(R.string.pair_cancelled))
    }

    private fun showPairing(code: String) {
        hideStatus()
        pairTitle.text = getString(R.string.pair_with, computer.title)
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
            if (session !== this@Session) return
            // The host's first packet is a keyframe and arrives right after this call on the
            // same thread, so the decoder has to exist before returning.
            remote.frameWidth = width
            remote.frameHeight = height
            frameSize = width to height
            startDecoder()
            runOnUiThread {
                if (session === this@Session) handleWelcome(hostFingerprint, hostName, width, height, opusHead)
            }
        }

        override fun onVideoSize(width: Int, height: Int) {
            if (session !== this@Session) return
            // As after the welcome, the first frame of the new size follows on this thread.
            remote.frameWidth = width
            remote.frameHeight = height
            frameSize = width to height
            startDecoder()
            runOnUiThread {
                if (session === this@Session) aspectFrame.setAspect(width, height)
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
        const val EXTRA_COMPUTER = "computer"

        /** For MainActivity: connect to this address (with [EXTRA_PORT]) when it starts. */
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        private const val TAG = "iolite"
        private const val MENU_WIDTH_DP = 240f
        private const val STATUS_PULSE_MS = 900L
        private const val RESIZE_SETTLE_MS = 400L
        private const val SCALE_PER_DENSITY = 0.75f
        private val RETRY_SECONDS = longArrayOf(1, 2, 4, 8, 16, 30)

        /** Part of the host's close reason when another device takes the session; retrying would take it back. */
        private const val TAKEN_OVER = "took over"
    }
}
