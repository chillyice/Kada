package com.kada.spike

import android.graphics.drawable.Icon
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/**
 * S2 手机档：快捷设置磁贴。
 *
 * 磁贴只做**一个开关**——「Kada 接管开 / 关」。手机上最常见的需求是「现在别拦我的键」，
 * 而不是在通知栏里跑宏；点一下暂停、再点恢复，状态在磁贴图标上直接可见（ACTIVE / INACTIVE）。
 *
 * 长按磁贴由系统打开本应用（`TileService` 的默认行为），不另做入口。
 */
class KadaTileService : TileService() {

    override fun onStartListening() {
        super.onStartListening()
        sync()
    }

    override fun onClick() {
        super.onClick()
        SpikeState.toggleMaster("tile")
        sync()
    }

    private fun sync() {
        val tile = qsTile ?: return
        tile.state = if (SpikeState.swallowMaster) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        tile.label = "Kada"
        tile.contentDescription = if (SpikeState.swallowMaster) "Kada 快捷键已接管" else "Kada 快捷键已暂停"
        tile.icon = Icon.createWithResource(this, R.drawable.ic_kada)
        tile.updateTile()
    }
}
