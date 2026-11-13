package com.kada.spike

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager

class KadaApp : Application() {
    override fun onCreate() {
        super.onCreate()
        SpikeState.init(this)
        // S1：把判定与配置交给 Rust（同一份 kada-core）。.so 缺失时内部降级，不影响 S0 实验。
        RustBridge.init(this)
        val manager = getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(HeartbeatService.CHANNEL_ID) == null) {
            val channel = NotificationChannel(
                HeartbeatService.CHANNEL_ID,
                "Kada S0 心跳",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "前台服务存活实验（S0-4）"
                setShowBadge(false)
                enableVibration(false)
            }
            manager.createNotificationChannel(channel)
        }
    }
}
