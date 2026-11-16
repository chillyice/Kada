package com.kada.spike

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * 系统安装器的回执接收器（`PackageInstaller` 会话的 status `PendingIntent`）。
 *
 * 只做一件事：把 `UPDATE_STATUS` / `EXTRA_STATUS_MESSAGE` 转给 [Update]，
 * 让界面与报告能看到「成功 / 被取消 / 失败原因」——**不在这里自己弹窗**。
 */
class UpdateReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Update.ACTION_STATUS) return
        SpikeState.init(context)
        Update.onInstallStatus(context, intent)
    }
}
