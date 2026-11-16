package com.kada.spike

/**
 * 触发键录入（S1-d）：编辑器点「录入」后，下一下按键被这里截走，拼成 core 的键名写法回填。
 *
 * 为什么必须在无障碍回调里截：**录入期间不能先执行一遍**——录 F8 时若让 Rust 判定先跑，
 * F8 原来的快捷键就先触发了一次（可能拉起浏览器抢前台，把录入打断）。
 *
 * 只录**组合键**（单键也算组合）：序列 / 和弦要多步状态，手机上录不如直接敲。
 * 录入期间按键一律吞：漏给 app 就会在目标输入框里留下半截字符。
 */
object TriggerCapture {

    /** 修饰键本身不结束录入（按 Ctrl 只是「准备按住」），只是被吞掉。 */
    private val modifierNames = setOf("Ctrl", "Alt", "Shift", "Meta")

    @Volatile
    private var sink: ((String) -> Unit)? = null

    /** 录到之后还要吞的键：等它自己的抬起到了才收手（否则 app 收到「只有 keyup」）。 */
    @Volatile
    private var awaitingRelease: String? = null

    val active: Boolean get() = sink != null

    /** 还在等录到的那个键抬起（收尾吞键阶段）。 */
    val pendingRelease: Boolean get() = awaitingRelease != null

    fun begin(listener: (String) -> Unit) {
        sink = listener
        awaitingRelease = null
    }

    fun cancel() {
        sink = null
        awaitingRelease = null
    }

    /**
     * 无障碍线程调用。返回 `true` = 吞掉该事件。
     *
     * `modsCsv` 是 `KeyNames.modifierCsv` 的产物（`Ctrl,Alt`），这里换成配置里的写法
     * （`Ctrl+Alt+K`），顺序与 core `format_shortcut` 一致。
     */
    fun offer(keyName: String, modsCsv: String, down: Boolean, repeat: Boolean): Boolean {
        val listener = sink
        if (listener == null) return drain(keyName, down)

        if (!down || repeat) return true
        if (keyName in modifierNames) return true

        val prefix = modsCsv.split(',').map { it.trim() }.filter { it.isNotEmpty() }
        val combo = (prefix + keyName).joinToString("+")

        // 录到就收工：连按两下不该录两次（第二下会写进输入框，成为「Ctrl+K+K」）。
        sink = null
        awaitingRelease = keyName
        listener(combo)
        return true
    }

    /**
     * 收尾：录到的那个键的抬起也吞掉，然后放行。
     *
     * 安全阀：等不到抬起（无障碍偶发丢事件）时，**新的非修饰键按下**就当作「用户已经在正常打字」，
     * 立刻停止吞键——宁可漏半个抬起，也不能把键盘永久吞住。
     */
    private fun drain(keyName: String, down: Boolean): Boolean {
        val pending = awaitingRelease ?: return false
        if (!down) {
            if (keyName == pending) awaitingRelease = null
            return true
        }
        if (keyName == pending || keyName in modifierNames) return true
        awaitingRelease = null
        return false
    }
}
