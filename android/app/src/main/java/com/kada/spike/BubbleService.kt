package com.kada.spike

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.IBinder
import android.provider.Settings
import android.util.Log
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import kotlin.math.abs

/**
 * S2 手机档：悬浮球 + 快捷面板（粘贴模板 / 打开应用的「点一下」入口）。
 *
 * 为什么是悬浮球而不是桌面那套浮层：Android 上 Tauri 多窗口不支持，`WindowManager` 的
 * `TYPE_APPLICATION_OVERLAY` 才是正路；手机没有键盘，鼠标手势 / 热键都无从谈起，**点一下**
 * 是唯一顺手的触发源。
 *
 * 三条边界：
 * 1. **要悬浮窗权限**（`SYSTEM_ALERT_WINDOW`）：没授权就不显示，状态里明说，引导去设置，
 *    不静默失败。
 * 2. **悬浮球与面板都不抢焦点**（`FLAG_NOT_FOCUSABLE`）：抢了焦点会把前台应用的输入连接断开，
 *    「插入模板」正好需要目标输入框还连着。
 * 3. **执行动作走无障碍服务**（`KeyFilterService.runShortcut`）：动作执行器挂在无障碍实例上，
 *    服务没连时明确报错（`lastRun` 里能看到），不假装成功。
 */
class BubbleService : Service() {

    private lateinit var windowManager: WindowManager
    private var bubble: View? = null
    private var panel: View? = null
    private var board: View? = null
    private var bubbleParams: WindowManager.LayoutParams? = null

    private var downRawX = 0f
    private var downRawY = 0f
    private var startX = 0
    private var startY = 0
    private var moved = false

    /** 长按判定（S2b）：按住不动 400ms = 开画板，拖动就作废。 */
    private var longPressTask: Runnable? = null
    private var longPressed = false

    override fun onCreate() {
        super.onCreate()
        SpikeState.init(this)
        windowManager = getSystemService(WindowManager::class.java)
        instance = this
        startForegroundCompat()
        addBubble()
        running = true
        Log.i(SpikeState.TAG, "bubble started permission=${hasPermission(this)}")
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopSelf()
                return START_NOT_STICKY
            }

            ACTION_SHOW_PANEL -> showPanel()
            ACTION_SHOW_BOARD -> showBoard()
        }
        return START_STICKY
    }

    override fun onDestroy() {
        hidePanel()
        hideBoard()
        bubble?.let { runCatching { windowManager.removeView(it) } }
        bubble = null
        bubbleParams = null
        running = false
        panelVisible = false
        boardVisible = false
        instance = null
        Log.i(SpikeState.TAG, "bubble stopped")
        super.onDestroy()
    }

    // ------------------------------------------------------------------
    // 悬浮球
    // ------------------------------------------------------------------

    private fun addBubble() {
        if (bubble != null) return
        val size = dp(56)
        val view = TextView(this).apply {
            text = "K"
            setTextColor(Color.WHITE)
            textSize = 22f
            gravity = Gravity.CENTER
            background = GradientDrawable().apply {
                shape = GradientDrawable.OVAL
                setColor(0xFFFF7A18.toInt())
                setStroke(dp(2), 0x33FFFFFF)
            }
            elevation = dp(6).toFloat()
            contentDescription = "Kada 悬浮球"
        }
        val params = WindowManager.LayoutParams(
            size,
            size,
            overlayType(),
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = prefs().getInt(KEY_X, dp(12))
            y = prefs().getInt(KEY_Y, dp(280))
        }

        view.setOnTouchListener { _, event -> onBubbleTouch(view, params, event) }
        view.setOnClickListener { togglePanel() }
        runCatching { windowManager.addView(view, params) }
            .onFailure {
                lastError = "悬浮球添加失败：${it.message}（检查悬浮窗权限）"
                Log.e(SpikeState.TAG, lastError)
                stopSelf()
                return
            }
        bubble = view
        bubbleParams = params
    }

    private fun onBubbleTouch(view: View, params: WindowManager.LayoutParams, event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downRawX = event.rawX
                downRawY = event.rawY
                startX = params.x
                startY = params.y
                moved = false
                longPressed = false
                // 长按 = 画符号（S2b）：点一下是面板，长按才是画板——两个入口不抢
                val task = Runnable {
                    if (!moved && board == null) {
                        longPressed = true
                        showBoard()
                    }
                }
                longPressTask = task
                view.postDelayed(task, LONG_PRESS_MS)
                return true
            }

            MotionEvent.ACTION_MOVE -> {
                val dx = (event.rawX - downRawX).toInt()
                val dy = (event.rawY - downRawY).toInt()
                if (!moved && (abs(dx) > dp(6) || abs(dy) > dp(6))) {
                    moved = true
                    cancelLongPress()
                }
                if (moved) {
                    params.x = (startX + dx).coerceAtLeast(0)
                    params.y = (startY + dy).coerceAtLeast(0)
                    runCatching { windowManager.updateViewLayout(view, params) }
                    positionPanel()
                }
                return true
            }

            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                cancelLongPress()
                if (event.actionMasked == MotionEvent.ACTION_CANCEL) {
                    moved = false
                    longPressed = false
                    return true
                }
                if (moved) {
                    prefs().edit().putInt(KEY_X, params.x).putInt(KEY_Y, params.y).apply()
                } else if (!longPressed) {
                    view.performClick()
                }
                longPressed = false
                return true
            }
        }
        return false
    }

    private fun cancelLongPress() {
        longPressTask?.let { bubble?.removeCallbacks(it) }
        longPressTask = null
    }

    // ------------------------------------------------------------------
    // 面板
    // ------------------------------------------------------------------

    private fun togglePanel() {
        if (panel != null) hidePanel() else showPanel()
    }

    private fun showPanel() {
        if (panel != null) return
        val entries = ShortcutStore.entries().filter { it.enabled }
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(14), dp(10), dp(14), dp(10))
            background = GradientDrawable().apply {
                cornerRadius = dp(16).toFloat()
                setColor(0xF21E1E1E.toInt())
            }
        }

        column.addView(
            TextView(this).apply {
                text = "Kada 面板 · ${entries.size} 条"
                setTextColor(Color.WHITE)
                textSize = 14f
            },
        )
        if (entries.isEmpty()) {
            column.addView(
                TextView(this).apply {
                    text = "配置里没有启用的快捷键——先在「快捷键编辑器」里加一条。"
                    setTextColor(0xFFBBBBBB.toInt())
                    textSize = 12f
                    setPadding(0, dp(8), 0, 0)
                },
            )
        }
        for (entry in entries) {
            val label = buildString {
                append(entry.name.ifBlank { "#${entry.index}（未命名）" })
                append('\n')
                append(entry.actionSummary)
            }
            column.addView(
                Button(this).apply {
                    text = label
                    textSize = 12f
                    isAllCaps = false
                    gravity = Gravity.START or Gravity.CENTER_VERTICAL
                    setPadding(dp(10), dp(4), dp(10), dp(4))
                    setOnClickListener {
                        hidePanel()
                        val ok = KeyFilterService.instance?.runShortcut(entry.index, "panel") ?: false
                        lastRun = if (ok) {
                            "panel:${entry.name.ifBlank { "#${entry.index}" }}"
                        } else {
                            "panel:${entry.name.ifBlank { "#${entry.index}" }}（无障碍服务未连接，没执行）"
                        }
                    }
                },
            )
        }
        column.addView(
            Button(this).apply {
                text = "关闭"
                textSize = 12f
                isAllCaps = false
                setOnClickListener { hidePanel() }
            },
        )

        val scroll = ScrollView(this).apply {
            addView(column)
            isFillViewport = false
        }
        val width = dp(268)
        val maxHeight = (resources.displayMetrics.heightPixels * 0.6).toInt()
        scroll.measure(
            View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(maxHeight, View.MeasureSpec.AT_MOST),
        )
        val params = WindowManager.LayoutParams(
            width,
            scroll.measuredHeight.coerceAtMost(maxHeight),
            overlayType(),
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
        }
        runCatching { windowManager.addView(scroll, params) }
            .onFailure {
                lastError = "面板添加失败：${it.message}"
                Log.e(SpikeState.TAG, lastError)
                return
            }
        panel = scroll
        panelVisible = true
        positionPanel()
    }

    private fun hidePanel() {
        panel?.let { runCatching { windowManager.removeView(it) } }
        panel = null
        panelVisible = false
    }

    /** 面板贴着悬浮球：空间够就放右侧，不够放左侧；纵向对齐后夹到屏幕内。 */
    private fun positionPanel() {
        val view = panel ?: return
        val b = bubbleParams ?: return
        val screenWidth = resources.displayMetrics.widthPixels
        val screenHeight = resources.displayMetrics.heightPixels
        val params = view.layoutParams as? WindowManager.LayoutParams ?: return
        val width = params.width
        val height = params.height
        var x = b.x + dp(64)
        if (x + width > screenWidth - dp(8)) x = (b.x - width - dp(8)).coerceAtLeast(dp(8))
        val y = b.y.coerceIn(dp(8), (screenHeight - height - dp(8)).coerceAtLeast(dp(8)))
        params.x = x
        params.y = y
        // FLAG_LAYOUT_NO_LIMITS 的窗口改坐标后必须重新 add/update 才生效
        runCatching { windowManager.updateViewLayout(view, params) }
    }

    // ------------------------------------------------------------------
    // 画板（S2b：长按悬浮球）
    // ------------------------------------------------------------------

    private fun showBoard() {
        if (board != null) return
        hidePanel()
        val view = DrawBoardView(this) { hideBoard() }
        val params = WindowManager.LayoutParams(
            WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.MATCH_PARENT,
            overlayType(),
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
        }
        runCatching { windowManager.addView(view, params) }
            .onFailure {
                lastError = "画板添加失败：${it.message}"
                Log.e(SpikeState.TAG, lastError)
                return
            }
        board = view
        boardVisible = true
        Log.i(SpikeState.TAG, "draw board shown")
    }

    private fun hideBoard() {
        board?.let { runCatching { windowManager.removeView(it) } }
        board = null
        boardVisible = false
    }

    // ------------------------------------------------------------------
    // 前台通知 / 工具
    // ------------------------------------------------------------------

    private fun startForegroundCompat() {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, BubbleService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification: Notification = Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_kada)
            .setContentTitle("Kada 悬浮球已开启")
            .setContentText("点一下打开面板 · 长按画符号 · 拖动可移动")
            .setContentIntent(open)
            .addAction(
                Notification.Action.Builder(
                    android.graphics.drawable.Icon.createWithResource(this, R.drawable.ic_kada),
                    "关闭悬浮球",
                    stop,
                ).build(),
            )
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun overlayType(): Int = if (Build.VERSION.SDK_INT >= 26) {
        WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
    } else {
        @Suppress("DEPRECATION")
        WindowManager.LayoutParams.TYPE_PHONE
    }

    private fun prefs() = getSharedPreferences("kada-bubble", Context.MODE_PRIVATE)

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        const val CHANNEL_ID = "kada-bubble"
        const val NOTIFICATION_ID = 1002
        const val ACTION_STOP = "com.kada.spike.STOP_BUBBLE"
        const val ACTION_SHOW_PANEL = "com.kada.spike.SHOW_BUBBLE_PANEL"

        /** S2b：画板也走 service intent，躲开「冷启动时 instance 还是 null」的竞态。 */
        const val ACTION_SHOW_BOARD = "com.kada.spike.SHOW_DRAW_BOARD"

        private const val KEY_X = "bubbleX"
        private const val KEY_Y = "bubbleY"

        /** 长按多久开画板（毫秒）：太短会和拖动 / 点击抢，太长手感又黏。 */
        const val LONG_PRESS_MS = 400L

        @Volatile
        var running = false
            private set

        @Volatile
        var panelVisible = false
            private set

        /** 画板浮层是否在显示（报告 / adb 断言读）。 */
        @Volatile
        var boardVisible = false
            private set

        /** 最近一次从面板执行的结果（报告 / adb 断言读这里）。 */
        @Volatile
        var lastRun = ""
            private set

        @Volatile
        var lastError = ""
            private set

        private var instance: BubbleService? = null

        fun hasPermission(context: Context): Boolean = Settings.canDrawOverlays(context)

        fun requestPermission(context: Context) {
            val intent = Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                android.net.Uri.parse("package:${context.packageName}"),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            runCatching { context.startActivity(intent) }
        }

        /** 启动悬浮球；没权限 / 无障碍没连都只记状态、不崩。返回是否已发出启动。 */
        fun start(context: Context): Boolean {
            if (!hasPermission(context)) {
                lastError = "没有悬浮窗权限：去「设置 → 应用 → Kada → 显示在其他应用上层」打开"
                return false
            }
            runCatching { context.startForegroundService(Intent(context, BubbleService::class.java)) }
                .onFailure {
                    lastError = "悬浮球启动失败：${it.message}"
                    return false
                }
            lastError = ""
            return true
        }

        fun stop(context: Context) {
            runCatching { context.stopService(Intent(context, BubbleService::class.java)) }
        }

        fun showPanel(context: Context) {
            if (!running) start(context)
            instance?.showPanel()
        }

        fun hidePanel() {
            instance?.hidePanel()
        }

        /**
         * 打开画板（S2b）：长按悬浮球走的是同一条路，adb / 应用内「试画」也走这里，
         * 这样脚本验的就是手指按出来的那个浮层，不是另一份实现。
         *
         * 为什么走 service intent 而不是直接 `instance?.showBoard()`：冷启动时
         * `startForegroundService` 是**异步**的，广播线程返回时 `instance` 还是 null，
         * 直接调就会「返回成功但什么也没弹」。交给 `onStartCommand` 就没有这个竞态
         * ——`onCreate` 一定先跑完。
         */
        fun showBoard(context: Context): Boolean {
            if (!hasPermission(context)) {
                lastError = "没有悬浮窗权限：去「设置 → 应用 → Kada → 显示在其他应用上层」打开"
                return false
            }
            val intent = Intent(context, BubbleService::class.java).setAction(ACTION_SHOW_BOARD)
            val ok = runCatching { context.startForegroundService(intent) }.isSuccess
            lastError = if (ok) "" else "画板打不开：悬浮球服务起不来"
            return ok
        }

        fun hideBoard() {
            instance?.hideBoard()
        }

        /**
         * 从悬浮球面板执行一条（与点按同一个入口）；服务没在跑也能执行动作，
         * 只是没有 UI——自动化脚本靠这条路验「点一下 → 插入模板 / 打开应用」。
         */
        fun runEntry(context: Context, index: Int, name: String): Boolean {
            val entry = ShortcutStore.entries().firstOrNull {
                (name.isNotEmpty() && it.name == name) || (name.isEmpty() && it.index == index)
            }
            val targetIndex = entry?.index ?: index
            val label = entry?.name?.ifBlank { "#$targetIndex" } ?: "#$targetIndex"
            val ok = KeyFilterService.instance?.runShortcut(targetIndex, "panel") ?: false
            lastRun = if (ok) "panel:$label" else "panel:$label（无障碍服务未连接，没执行）"
            return ok
        }
    }
}
