package com.kada.spike

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.Log

/**
 * S0-4 的探针：前台服务 + 常驻通知 + 心跳日志。
 *
 * 它不持有引擎——Android 上「服务活着但进程被冻」与「进程活着但无障碍被摘」
 * 是两件事，S0 分开测：本服务只证明「进程常驻」这一半。
 */
class HeartbeatService : Service() {

    private val handler = Handler(Looper.getMainLooper())
    private var started = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            Log.i(SpikeState.TAG, "heartbeat stop requested")
            stopSelf()
            return START_NOT_STICKY
        }
        startForegroundCompat()
        if (!started) {
            started = true
            SpikeState.onHeartbeatStarted()
            handler.post(tick)
        }
        if (intent?.action == ACTION_TOGGLE) {
            // S2 通知动作：暂停 / 恢复与磁贴、面板同一个总闸
            SpikeState.toggleMaster("notify")
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, buildNotification(currentText()))
        }
        return START_STICKY
    }

    override fun onDestroy() {
        if (started) {
            started = false
            handler.removeCallbacks(tick)
            SpikeState.onHeartbeatStopped()
        }
        super.onDestroy()
    }

    private fun startForegroundCompat() {
        val notification = buildNotification("启动中…")
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private val tick = object : Runnable {
        override fun run() {
            SpikeState.heartbeatTick()
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, buildNotification(currentText()))
            val stats = SpikeState.stats()
            val latency = SpikeState.latencySummary()
            Log.i(
                SpikeState.TAG,
                "hb tick=${SpikeState.heartbeatTicks} min=%.1f keys=%d swallowed=%d actions=%d p50=%.2f"
                    .format(minutes(), stats.down, stats.swallowed, stats.actionsFired, latency.p50),
            )
            handler.postDelayed(this, TICK_MS)
        }
    }

    private fun minutes(): Double = if (SpikeState.heartbeatStartedElapsed == 0L) 0.0
    else (SystemClock.elapsedRealtime() - SpikeState.heartbeatStartedElapsed) / 60_000.0

    private fun currentText(): String {
        val stats = SpikeState.stats()
        val state = if (SpikeState.swallowMaster) "接管中" else "已暂停"
        return "已存活 %.1f 分钟 · %s · 键 %d · 吞 %d · 动作 %d"
            .format(minutes(), state, stats.down, stats.swallowed, stats.actionsFired)
    }

    private fun buildNotification(text: String): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val toggle = PendingIntent.getService(
            this,
            2,
            Intent(this, HeartbeatService::class.java).setAction(ACTION_TOGGLE),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, HeartbeatService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val toggleAction = Notification.Action.Builder(
            Icon.createWithResource(this, R.drawable.ic_kada),
            if (SpikeState.swallowMaster) "暂停接管" else "恢复接管",
            toggle,
        ).build()
        val stopAction = Notification.Action.Builder(
            Icon.createWithResource(this, R.drawable.ic_kada),
            "停止",
            stop,
        ).build()
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_kada)
            .setContentTitle(if (SpikeState.swallowMaster) "Kada · 已接管" else "Kada · 已暂停")
            .setContentText(text)
            .setContentIntent(open)
            .addAction(toggleAction)
            .addAction(stopAction)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
    }

    companion object {
        const val CHANNEL_ID = "kada-s0-heartbeat"
        const val NOTIFICATION_ID = 1001
        const val ACTION_STOP = "com.kada.spike.STOP_HEARTBEAT"
        const val ACTION_TOGGLE = "com.kada.spike.TOGGLE_MASTER"
        private const val TICK_MS = 30_000L
    }
}
