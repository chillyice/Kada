package com.kada.spike

import android.view.KeyEvent

/**
 * Android `KeyEvent.keyCode` ↔ kada-core 中性键名（说明书 §2 键模型）。
 *
 * 只做映射，不做判定：`Key::ALL` 与前端键表的一致性由 Rust 侧守着，这里只是
 * S0 探针要能在报告里用「Ctrl+Alt+K」这种可读名写日志。S1 搬 core 时这张表
 * 要原样落到 `crates/kada-core/src/keys.rs` 的 Android 分支（docs/安卓版规划.md §5.1）。
 */
object KeyNames {
    private val names: Map<Int, String> = buildMap {
        for (i in 0..25) put(KeyEvent.KEYCODE_A + i, ('A' + i).toString())
        for (i in 0..9) put(KeyEvent.KEYCODE_0 + i, i.toString())
        for (i in 0..11) put(KeyEvent.KEYCODE_F1 + i, "F${i + 1}")
        for (i in 0..9) put(KeyEvent.KEYCODE_NUMPAD_0 + i, "Numpad$i")

        put(KeyEvent.KEYCODE_COMMA, ",")
        put(KeyEvent.KEYCODE_PERIOD, ".")
        put(KeyEvent.KEYCODE_SLASH, "/")
        put(KeyEvent.KEYCODE_BACKSLASH, "\\")
        put(KeyEvent.KEYCODE_SEMICOLON, ";")
        put(KeyEvent.KEYCODE_APOSTROPHE, "\"")
        put(KeyEvent.KEYCODE_GRAVE, "`")
        put(KeyEvent.KEYCODE_MINUS, "-")
        put(KeyEvent.KEYCODE_EQUALS, "=")
        put(KeyEvent.KEYCODE_LEFT_BRACKET, "[")
        put(KeyEvent.KEYCODE_RIGHT_BRACKET, "]")

        put(KeyEvent.KEYCODE_ENTER, "Enter")
        put(KeyEvent.KEYCODE_ESCAPE, "Esc")
        put(KeyEvent.KEYCODE_TAB, "Tab")
        put(KeyEvent.KEYCODE_SPACE, "Space")
        put(KeyEvent.KEYCODE_DEL, "Backspace")
        put(KeyEvent.KEYCODE_FORWARD_DEL, "Delete")
        put(KeyEvent.KEYCODE_INSERT, "Insert")
        put(KeyEvent.KEYCODE_CAPS_LOCK, "CapsLock")

        put(KeyEvent.KEYCODE_SHIFT_LEFT, "Shift")
        put(KeyEvent.KEYCODE_SHIFT_RIGHT, "Shift")
        put(KeyEvent.KEYCODE_CTRL_LEFT, "Ctrl")
        put(KeyEvent.KEYCODE_CTRL_RIGHT, "Ctrl")
        put(KeyEvent.KEYCODE_ALT_LEFT, "Alt")
        put(KeyEvent.KEYCODE_ALT_RIGHT, "Alt")
        put(KeyEvent.KEYCODE_META_LEFT, "Meta")
        put(KeyEvent.KEYCODE_META_RIGHT, "Meta")

        put(KeyEvent.KEYCODE_MOVE_HOME, "Home")
        put(KeyEvent.KEYCODE_MOVE_END, "End")
        put(KeyEvent.KEYCODE_PAGE_UP, "PageUp")
        put(KeyEvent.KEYCODE_PAGE_DOWN, "PageDown")
        put(KeyEvent.KEYCODE_DPAD_UP, "Up")
        put(KeyEvent.KEYCODE_DPAD_DOWN, "Down")
        put(KeyEvent.KEYCODE_DPAD_LEFT, "Left")
        put(KeyEvent.KEYCODE_DPAD_RIGHT, "Right")

        put(KeyEvent.KEYCODE_VOLUME_UP, "VolumeUp")
        put(KeyEvent.KEYCODE_VOLUME_DOWN, "VolumeDown")
        put(KeyEvent.KEYCODE_VOLUME_MUTE, "VolumeMute")
        put(KeyEvent.KEYCODE_MUTE, "VolumeMute")
        put(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, "MediaPlayPause")
        put(KeyEvent.KEYCODE_MEDIA_PREVIOUS, "MediaPrev")
        put(KeyEvent.KEYCODE_MEDIA_NEXT, "MediaNext")

        put(KeyEvent.KEYCODE_NUMPAD_ADD, "NumpadAdd")
        put(KeyEvent.KEYCODE_NUMPAD_SUBTRACT, "NumpadSubtract")
        put(KeyEvent.KEYCODE_NUMPAD_MULTIPLY, "NumpadMultiply")
        put(KeyEvent.KEYCODE_NUMPAD_DIVIDE, "NumpadDivide")
        put(KeyEvent.KEYCODE_NUMPAD_DOT, "NumpadDecimal")
        put(KeyEvent.KEYCODE_NUM_LOCK, "NumLock")
    }

    /** 没进表的一律回退到系统名（去掉 `KEYCODE_` 前缀），报告里不丢信息。 */
    fun name(keyCode: Int): String =
        names[keyCode] ?: KeyEvent.keyCodeToString(keyCode).removePrefix("KEYCODE_")

    fun isCtrl(keyCode: Int) = keyCode == KeyEvent.KEYCODE_CTRL_LEFT || keyCode == KeyEvent.KEYCODE_CTRL_RIGHT

    fun isAlt(keyCode: Int) = keyCode == KeyEvent.KEYCODE_ALT_LEFT || keyCode == KeyEvent.KEYCODE_ALT_RIGHT

    fun isVolume(keyCode: Int) = keyCode == KeyEvent.KEYCODE_VOLUME_UP || keyCode == KeyEvent.KEYCODE_VOLUME_DOWN

    /** metaState 的中性名摘要，日志里看「这一下带没带修饰键」。 */
    fun modifierSummary(metaState: Int): String {
        val parts = ArrayList<String>(4)
        if (metaState and KeyEvent.META_CTRL_ON != 0) parts.add("Ctrl")
        if (metaState and KeyEvent.META_ALT_ON != 0) parts.add("Alt")
        if (metaState and KeyEvent.META_SHIFT_ON != 0) parts.add("Shift")
        if (metaState and KeyEvent.META_META_ON != 0) parts.add("Meta")
        return if (parts.isEmpty()) "-" else parts.joinToString("+")
    }

    /** 逗号分隔的修饰键（Rust 侧 `parse_mods` 的输入格式）；没修饰键时为空串。 */
    fun modifierCsv(metaState: Int): String {
        val parts = ArrayList<String>(4)
        if (metaState and KeyEvent.META_CTRL_ON != 0) parts.add("Ctrl")
        if (metaState and KeyEvent.META_ALT_ON != 0) parts.add("Alt")
        if (metaState and KeyEvent.META_SHIFT_ON != 0) parts.add("Shift")
        if (metaState and KeyEvent.META_META_ON != 0) parts.add("Meta")
        return parts.joinToString(",")
    }
}
