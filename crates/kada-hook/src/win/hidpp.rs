//! 罗技 HID++ 客户端（`docs/底层输入能力规划.md` 档位 C）。
//!
//! 解决「按了没反应」里 Kada 自己也无能为力的那一类：厂商代理（Logi Options+ 等）通过
//! HID++ 的 `REPROG_CONTROLS_V4` 把某个控件 **divert** 给自己，设备从此不再把这个键放进
//! 标准 HID 报文——Windows 输入栈里根本没有这次按下，低层钩子拿不到。
//!
//! 本模块只做一件事：**把 divert 关掉**（`setCidReporting(cid, divert=false)`），让按键
//! 回到标准报文本，现成的钩子链直接看见它。**不接管事件流**——Kada 退出后按键仍是原生行为，
//! 延迟也与现状一致（取舍理由见该文档档位 C）。
//!
//! 边界：
//! - 只管罗技自家设备（HID++ 是罗技的私协议）；其它厂商的接管仍走档位 A 的文字指引。
//! - 只动 Kada 键模型里的三个鼠标键（中键 / 后退 / 前进 = CID `0x0052` / `0x0053` /
//!   `0x0056`）：手势键、SmartShift 等控件解除 divert 会破坏用户在厂商软件里的设置，不做。
//! - 通道打不开（被独占）就报错，界面退化为档位 A 的指引，**不报「已解锁」**。
//!
//! 协议要点（与 Solaar 的实现一致）：
//! - 报文 `[报告号, 设备索引, 功能索引, 功能号<<4|软件ID, 参数...]`；短报文 `0x10`（7 字节）、
//!   长报文 `0x11`（20 字节）。回包按「功能索引 + 功能号|软件ID」精确匹配。
//! - 错误回包布局是 `[0xFF, 功能索引, 功能号|ID, 错误码]`（错误码在参数区第 1 字节）。
//! - 根功能索引固定 `0x00`：`getFeature(0x1B04)` 拿到 `REPROG_CONTROLS_V4` 的功能索引。
//! - 设备索引：直连（USB 线 / 蓝牙）是 `0xFF`，接收器后面的外设是 `1..=6`。
//! - `getCidReporting` 回包：`cid(2) + flags1(1) + 映射目标(2) + flags2(1)`，divert 位在
//!   `flags1` bit0，持久 divert 位在 `flags1` bit2；`setCidReporting` 的第三字节是
//!   「要改哪些位」的 valid 掩码（位值左移一位）。

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::hidpp::{ControlStatus, DeviceStatus, Status, UnlockDevice, UnlockReport};

/// 罗技（Logitech）USB 厂商号。
const LOGITECH_VID: u16 = 0x046D;
/// HID++ 控制集合的 usage page（vendor-defined）。
const HIDPP_USAGE_PAGE: u16 = 0xFF00;
const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
const SHORT_LEN: usize = 7;
const LONG_LEN: usize = 20;

/// 根功能索引（所有 HID++ 2.0 设备都有）。
const ROOT_FEATURE: u8 = 0x00;
/// `REPROG_CONTROLS_V4`（可重编程控件，含 divert 能力）。
const REPROG_CONTROLS_V4: u16 = 0x1B04;

/// 软件 ID（放在功能号低 4 位）。最高位必须置 1，设备才把它当「请求」而不是通知
/// （Solaar 的做法）；具体取值只用于回包匹配，取一个不常见的即可。
const SWID: u8 = 0x0C;

const FN_GET_COUNT: u8 = 0x00;
const FN_GET_REPORTING: u8 = 0x02;
const FN_SET_REPORTING: u8 = 0x03;

/// `getCidReporting` 的 flags1：divert。
const FLAG_DIVERT: u8 = 0x01;
/// `getCidReporting` 的 flags1：持久 divert（重启 / 重连后仍生效）。
const FLAG_PERSIST: u8 = 0x04;

/// 单次请求等回包的上限。设备休眠唤醒后第一包可能慢一拍，故不取太小。
const REQ_TIMEOUT: Duration = Duration::from_millis(600);
/// 探测接收器后面的空槽位：没设备时只能等超时，取短一点（六个槽位最坏也只多等 ~2 秒）。
const SLOT_TIMEOUT: Duration = Duration::from_millis(300);

/// 保持解锁的巡检间隔。设备重连 / 代理重启会把 divert 设回，这个周期决定「失而复得」多快。
const KEEP_INTERVAL: Duration = Duration::from_secs(10);
/// 设备断了之后重新枚举的间隔（重连可能换了槽位，只能整段重来）。
const RECONNECT_INTERVAL: Duration = Duration::from_secs(30);
/// 重连尝试上限（× `RECONNECT_INTERVAL` ≈ 10 分钟）：超过就放弃并提示用户重新点解锁。
const RECONNECT_ATTEMPTS: u32 = 20;

/// Kada 键模型里的三个鼠标键（CID 与 Solaar `special_keys.CONTROL` 一致）。
const MOUSE_CIDS: [(u16, &str); 3] = [
    (0x0052, "中键"),
    (0x0053, "后退侧键"),
    (0x0056, "前进侧键"),
];

// ---- HID++ 低层 ----

#[derive(Debug)]
enum LinkError {
    /// 等回包超时（设备不应答：休眠 / 槽位空）。
    Timeout,
    /// 设备回的 HID++ 错误码（功能不支持 / 参数不合法等）。
    Device(u8),
    /// HID 读写失败（设备拔出、句柄失效）。
    Io(String),
    /// 当前通道（短报文集合）放不下这条请求。
    TooLong,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Timeout => write!(f, "设备没有回应（可能休眠或该槽位没有设备）"),
            LinkError::Device(code) => write!(f, "设备返回错误码 0x{code:02X}"),
            LinkError::Io(e) => write!(f, "HID 通道读写失败：{e}"),
            LinkError::TooLong => write!(f, "设备只提供了短报文通道，放不下这条请求"),
        }
    }
}

/// 一条打通的 HID++ 通道 + 它指向的设备。
struct Link {
    dev: HidDevice,
    /// 设备的 HID++ 索引（`0xFF` = 直连）。
    index: u8,
    /// `REPROG_CONTROLS_V4` 的功能索引。
    feat: u8,
    /// 通道报告长度：长报文（20）还是短报文（7）。
    long: bool,
    product_id: u16,
    /// 展示名。
    name: String,
}

impl Link {
    /// 发一条 HID++ 请求并等匹配的回包（返回参数区）。
    ///
    /// 匹配条件严格按「功能索引 + 功能号|软件ID」：同一个接收器上可能同时有代理的
    /// 请求与设备通知在流动，只认自己的那一包。
    fn request(
        &self,
        feat: u8,
        func: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, LinkError> {
        let func_byte = (func << 4) | SWID;
        let len = if self.long { LONG_LEN } else { SHORT_LEN };
        // 短报文只有 3 个参数位：`setCidReporting`（5 字节参数）放不下。宁可报错，
        // 也不能截断成一条语义不同的请求写下去。
        if params.len() > len - 4 {
            return Err(LinkError::TooLong);
        }
        let mut frame = vec![0u8; len];
        frame[0] = if self.long { REPORT_LONG } else { REPORT_SHORT };
        frame[1] = self.index;
        frame[2] = feat;
        frame[3] = func_byte;
        frame[4..4 + params.len()].copy_from_slice(params);
        self.dev
            .write(&frame)
            .map_err(|e| LinkError::Io(e.to_string()))?;

        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; LONG_LEN];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(LinkError::Timeout);
            }
            let wait = left.as_millis().min(i32::MAX as u128) as i32;
            let got = match self.dev.read_timeout(&mut buf, wait) {
                Ok(n) => n,
                Err(e) => return Err(LinkError::Io(e.to_string())),
            };
            if got == 0 {
                continue;
            }
            let reply = &buf[..got];
            if reply.len() < 5 {
                continue;
            }
            let devno = reply[1];
            // 蓝牙直连设备有时用 0x00 作为设备号（0xFF 取反）。
            if devno != self.index && devno != (self.index ^ 0xFF) {
                continue;
            }
            let data = &reply[2..];
            if data.len() < 3 {
                continue;
            }
            if data[0] == 0xFF {
                // 错误回包：[0xFF, 功能索引, 功能号|ID, 错误码]
                if data.len() >= 4 && data[1] == feat && data[2] == func_byte {
                    return Err(LinkError::Device(data[3]));
                }
                continue;
            }
            if data[0] == feat && data[1] == func_byte {
                return Ok(data[2..].to_vec());
            }
            // 别人的回包 / 设备通知：跳过，继续等。
        }
    }

    /// 根功能 `getFeature(feature_id)` → 功能索引（0 = 设备没有这个功能）。
    fn get_feature(&self, feature_id: u16, timeout: Duration) -> Result<u8, LinkError> {
        let reply = self.request(
            ROOT_FEATURE,
            0x00,
            &[(feature_id >> 8) as u8, feature_id as u8],
            timeout,
        )?;
        Ok(reply.first().copied().unwrap_or(0))
    }

    /// 读某个控件的 reporting 标志（divert / persist）。
    fn get_reporting(&self, cid: u16) -> Result<(bool, bool), LinkError> {
        let reply = self.request(
            self.feat,
            FN_GET_REPORTING,
            &[(cid >> 8) as u8, cid as u8],
            REQ_TIMEOUT,
        )?;
        // 回包：cid(2) + flags1(1) + 映射目标(2) + flags2(1)
        let flags = reply.get(2).copied().unwrap_or(0);
        Ok((flags & FLAG_DIVERT != 0, flags & FLAG_PERSIST != 0))
    }

    /// 把某个控件的 divert 关掉（持久 divert 位一并清掉，否则设备可能带着旧配置复活）。
    fn clear_reporting(&self, cid: u16) -> Result<(), LinkError> {
        // valid 掩码 = 位值 << 1：divert=0x01 → 0x02；persist=0x04 → 0x08。
        // 值本身为 0（关掉），只置 valid 位。
        let mut mask = FLAG_DIVERT << 1;
        if self.get_reporting(cid).map(|(_, p)| p).unwrap_or(false) {
            mask |= FLAG_PERSIST << 1;
        }
        let reply = self.request(
            self.feat,
            FN_SET_REPORTING,
            &[(cid >> 8) as u8, cid as u8, mask, 0, 0],
            REQ_TIMEOUT,
        )?;
        // 设备会把请求原样回显；回显里 cid 对不上就说明没生效。
        let echo_cid = u16::from_be_bytes([
            reply.first().copied().unwrap_or(0),
            reply.get(1).copied().unwrap_or(0),
        ]);
        if echo_cid != 0 && echo_cid != cid {
            return Err(LinkError::Device(0));
        }
        Ok(())
    }

    /// 控件总数（`REPROG_CONTROLS_V4` 功能 0）。
    fn control_count(&self) -> u8 {
        self.request(self.feat, FN_GET_COUNT, &[], REQ_TIMEOUT)
            .ok()
            .and_then(|r| r.first().copied())
            .unwrap_or(0)
    }
}

/// 候选 HID 集合（同一个物理接口会暴露短 / 长 / 超长三个集合）。
struct Candidate {
    path: CString,
    usage: u16,
    /// USB 接口号：同一个物理接口的短 / 长集合共用它（去重用）。
    interface: i32,
    product_id: u16,
    product: Option<String>,
}

/// 按「长报文优先」排序候选集合。
fn candidates(api: &HidApi) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = api
        .device_list()
        .filter(|d| d.vendor_id() == LOGITECH_VID && d.usage_page() == HIDPP_USAGE_PAGE)
        .map(|d| Candidate {
            path: d.path().to_owned(),
            usage: d.usage(),
            interface: d.interface_number(),
            product_id: d.product_id(),
            product: d.product_string().map(str::to_string),
        })
        .collect();
    // usage 0x0002 = 长报文集合（20 字节），0x0001 = 短报文，0x0003 = 超长。
    out.sort_by_key(|c| match c.usage {
        0x0002 => 0,
        0x0001 => 1,
        _ => 2,
    });
    out
}

/// 打开一条通道并在设备索引上探测 `REPROG_CONTROLS_V4`。
///
/// `index` 为 None 时表示「自动找」：先试直连 `0xFF`，再试接收器的 1..=6。
fn open_link(api: &HidApi, cand: &Candidate, index: Option<u8>) -> Result<Link, LinkError> {
    // 先按集合类型猜报告长度；猜错了（集合与 usage 对不上）再换另一种试一次。
    let prefer_long = cand.usage != 0x0001;
    let mut last = LinkError::Timeout;
    for long in [prefer_long, !prefer_long] {
        let dev = api
            .open_path(&cand.path)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        let mut link = Link {
            dev,
            index: index.unwrap_or(0xFF),
            feat: 0,
            long,
            product_id: cand.product_id,
            name: cand.product.clone().unwrap_or_else(|| "罗技设备".to_string()),
        };
        match probe_link(&mut link, index) {
            Ok(()) => return Ok(link),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 在一条候选通道上确认 HID++ 能用，并定下设备索引。
///
/// 一个接收器后面可能挂着多个设备（键盘占槽位 1、鼠标在槽位 2）：优先选**有鼠标控件**的
/// 那个索引，找不到才退回「第一个支持 `REPROG_CONTROLS_V4` 的设备」。
fn probe_link(link: &mut Link, wanted: Option<u8>) -> Result<(), LinkError> {
    let indexes: Vec<u8> = match wanted {
        Some(i) => vec![i],
        None => {
            let mut v = vec![0xFF];
            v.extend(1..=6);
            v
        }
    };
    let mut fallback: Option<(u8, u8)> = None;
    for idx in indexes {
        link.index = idx;
        let timeout = if idx == 0xFF {
            REQ_TIMEOUT
        } else {
            SLOT_TIMEOUT
        };
        // 休眠设备第一包常超时，重试一次再放弃。
        let feature = match link.get_feature(REPROG_CONTROLS_V4, timeout) {
            Ok(f) => Ok(f),
            Err(LinkError::Timeout) => link.get_feature(REPROG_CONTROLS_V4, timeout),
            Err(e) => Err(e),
        };
        match feature {
            Ok(f) if f != 0 => {
                link.feat = f;
                let has_mouse = MOUSE_CIDS
                    .iter()
                    .any(|(cid, _)| link.get_reporting(*cid).is_ok());
                if has_mouse {
                    return Ok(());
                }
                fallback.get_or_insert((idx, f));
            }
            Ok(_) => continue,
            Err(LinkError::Device(_)) => continue,
            Err(LinkError::Timeout) => continue,
            Err(e) => return Err(e),
        }
    }
    if let Some((idx, feat)) = fallback {
        link.index = idx;
        link.feat = feat;
        return Ok(());
    }
    Err(LinkError::Timeout)
}

/// 枚举本机所有能说 HID++ 的罗技设备。返回「通道 + 设备索引」列表。
fn discover(api: &HidApi) -> (Vec<Link>, Vec<String>) {
    let mut links: Vec<Link> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    // 同一个物理接口的长 / 短集合会指向同一台设备：按 (产品号, 接口号, 设备索引) 去重，
    // 长通道先到（排序保证了这一点）。
    let mut seen: Vec<(u16, i32, u8)> = Vec::new();
    for cand in candidates(api) {
        match open_link(api, &cand, None) {
            Ok(link) => {
                let key = (link.product_id, cand.interface, link.index);
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                links.push(link);
            }
            Err(LinkError::Io(e)) => {
                // 打不开是常见情况（集合被独占 / 不是 HID++ 集合），只记不报错。
                notes.push(format!(
                    "{}（usage 0x{:04X}）打不开：{e}",
                    cand.product.as_deref().unwrap_or("罗技设备"),
                    cand.usage
                ));
            }
            Err(_) => {}
        }
    }
    (links, notes)
}

// ---- 保持解锁 ----

/// 保持解锁的巡检线程代号：递增即让旧线程退出（新一次解锁会换一批句柄）。
static KEEP_GEN: AtomicU64 = AtomicU64::new(0);
/// 是否有巡检线程在跑。
static KEEPER_ACTIVE: AtomicBool = AtomicBool::new(false);
/// 巡检线程最近一次错误（给设置页显示）。
static KEEPER_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn set_keeper_error(err: Option<String>) {
    if let Ok(mut g) = KEEPER_ERROR.lock() {
        *g = err;
    }
}

/// 解锁并开始保持。
///
/// 保持的策略：线程拿着刚才那批句柄每 [`KEEP_INTERVAL`] 看一眼 divert 位，被设回就再关掉；
/// 句柄失效（设备拔了 / 代理重启抢走）就每 [`RECONNECT_INTERVAL`] 重新枚举一次，直到重新
/// 接上。**只在本进程生命周期内有效**——Kada 退出后不再干预设备（这也是「不接管事件流」的
/// 同一取舍：用户随时可以彻底放弃这个功能）。
fn install_keeper(links: Vec<Link>) {
    if links.is_empty() {
        return;
    }
    let gen = KEEP_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    KEEPER_ACTIVE.store(true, Ordering::SeqCst);
    set_keeper_error(None);
    thread::spawn(move || {
        let mut links = links;
        let mut next_reconnect = Instant::now();
        // 设备真的拔掉后，重连尝试有上限：无线上网本来就是「拔了可能很久不插回来」，
        // 无限每 30 秒枚举一次 SetupAPI 不值得。放弃时把原因写进状态，用户再点一次解锁即可。
        let mut attempts = 0u32;
        loop {
            thread::sleep(KEEP_INTERVAL);
            if KEEP_GEN.load(Ordering::SeqCst) != gen {
                // 被更新的解锁取代：丢掉自己的句柄走人。**不要动 `KEEPER_ACTIVE`**——
                // 那个旗标现在归新线程，这里写 false 会把它刚置的 true 抹掉。
                return;
            }
            if links.is_empty() {
                if attempts >= RECONNECT_ATTEMPTS {
                    KEEPER_ACTIVE.store(false, Ordering::SeqCst);
                    set_keeper_error(Some(
                        "设备已断开较久，保持解锁已停止；重新接上后请再点一次「解锁侧键」".to_string(),
                    ));
                    return;
                }
                if Instant::now() >= next_reconnect {
                    attempts += 1;
                    next_reconnect = Instant::now() + RECONNECT_INTERVAL;
                    if let Ok(api) = HidApi::new() {
                        let (fresh, notes) = discover(&api);
                        links = fresh;
                        if links.is_empty() {
                            set_keeper_error(Some(if notes.is_empty() {
                                "设备暂时找不到（可能已断开）".to_string()
                            } else {
                                notes.join("；")
                            }));
                        } else {
                            attempts = 0;
                            KEEPER_ACTIVE.store(true, Ordering::SeqCst);
                            set_keeper_error(None);
                        }
                    } else {
                        set_keeper_error(Some("HID 枚举失败".to_string()));
                    }
                }
                continue;
            }
            let mut alive: Vec<Link> = Vec::new();
            let mut err: Option<String> = None;
            for link in links.drain(..) {
                let mut broken = false;
                for (cid, _) in MOUSE_CIDS {
                    match link.get_reporting(cid) {
                        Ok((true, _)) => {
                            if let Err(e) = link.clear_reporting(cid) {
                                // 超时 = 无线设备正好休眠，句柄还活着，下拍再说。
                                if !matches!(e, LinkError::Timeout) {
                                    err = Some(format!("{}：{e}", link.name));
                                }
                                broken = matches!(e, LinkError::Io(_));
                            }
                        }
                        Ok((false, _)) => {}
                        // 控件不存在（错误码）不算通道坏了；休眠超时也不算——只有 IO 错
                        // （句柄失效 / 设备拔出）才丢掉这条通道去重连。
                        Err(LinkError::Device(_)) | Err(LinkError::Timeout) => {}
                        Err(e) => {
                            err = Some(format!("{}：{e}", link.name));
                            broken = true;
                        }
                    }
                    if broken {
                        break;
                    }
                }
                if !broken {
                    alive.push(link);
                }
            }
            links = alive;
            set_keeper_error(err);
            if links.is_empty() {
                KEEPER_ACTIVE.store(false, Ordering::SeqCst);
            }
        }
    });
}

// ---- 对外入口 ----

/// 本平台是否实现了这套能力（供壳层填 `supported`；恒 true，只有 Windows 会编这个模块）。
pub fn supported() -> bool {
    true
}

/// 只读探测：列出本机罗技设备与三个鼠标键的 divert 状态。**不写设备**。
pub fn status() -> Status {
    let mut out = Status {
        supported: true,
        keeper_active: KEEPER_ACTIVE.load(Ordering::SeqCst),
        keeper_error: KEEPER_ERROR.lock().ok().and_then(|g| g.clone()),
        ..Status::default()
    };
    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            out.error = Some(format!("HID 枚举失败：{e}"));
            return out;
        }
    };
    let (links, notes) = discover(&api);
    if links.is_empty() && !notes.is_empty() {
        out.error = Some(notes.join("；"));
    }
    for link in &links {
        let mut controls = Vec::new();
        for (cid, label) in MOUSE_CIDS {
            match link.get_reporting(cid) {
                Ok((diverted, persistent)) => controls.push(ControlStatus {
                    cid,
                    label,
                    present: true,
                    diverted,
                    persistent,
                }),
                // 错误码 = 设备上没有这个控件；超时按「有但读不到」记一笔。
                Err(LinkError::Device(_)) => controls.push(ControlStatus {
                    cid,
                    label,
                    present: false,
                    diverted: false,
                    persistent: false,
                }),
                Err(_) => controls.push(ControlStatus {
                    cid,
                    label,
                    present: true,
                    diverted: false,
                    persistent: false,
                }),
            }
        }
        out.devices.push(DeviceStatus {
            name: device_label(link),
            product_id: link.product_id,
            index: link.index,
            control_count: link.control_count(),
            controls,
            channel: if link.long { "long" } else { "short" },
        });
    }
    // 保持解锁线程里不能同时再枚举一份句柄（同一个集合可以共享，但没必要多开），
    // 这里只负责展示状态。
    out
}

/// 解锁：把所有处于 divert 的鼠标键交回标准报文，并在成功后开始后台保持。
pub fn unlock() -> UnlockReport {
    let mut report = UnlockReport::default();
    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            report.error = Some(format!("HID 枚举失败：{e}"));
            return report;
        }
    };
    let (links, notes) = discover(&api);
    if links.is_empty() {
        report.error = Some(if notes.is_empty() {
            "没有找到可用的罗技 HID++ 设备（没接罗技设备，或控制接口打不开）".to_string()
        } else {
            format!("罗技控制接口打不开：{}", notes.join("；"))
        });
        return report;
    }
    let mut keep: Vec<Link> = Vec::new();
    for link in links {
        let mut dev = UnlockDevice {
            name: device_label(&link),
            ..UnlockDevice::default()
        };
        let mut touched = false;
        for (cid, label) in MOUSE_CIDS {
            match link.get_reporting(cid) {
                Ok((diverted, _)) => {
                    if !diverted {
                        dev.already.push(label);
                        continue;
                    }
                    match link.clear_reporting(cid) {
                        Ok(()) => {
                            // 写完回读确认：设备有可能拒绝（标志位不支持）。
                            match link.get_reporting(cid) {
                                Ok((false, _)) => {
                                    dev.cleared.push(label);
                                    touched = true;
                                }
                                Ok((true, _)) => {
                                    dev.failed.push(format!("{label}：写入了但设备仍报 divert"));
                                }
                                Err(e) => dev.failed.push(format!("{label}：回读失败（{e}）")),
                            }
                        }
                        Err(e) => dev.failed.push(format!("{label}：{e}")),
                    }
                }
                Err(LinkError::Device(_)) => dev.missing.push(label),
                Err(e) => dev.failed.push(format!("{label}：{e}")),
            }
        }
        if touched {
            keep.push(link);
        }
        report.devices.push(dev);
    }
    if report.devices.iter().any(|d| !d.cleared.is_empty()) {
        install_keeper(keep);
    }
    report
}

/// 设备展示名：产品名 + 槽位（接收器后面要能分清是哪一台）。
fn device_label(link: &Link) -> String {
    if link.index == 0xFF {
        format!("{}（直连）", link.name)
    } else {
        format!("{}（接收器槽位 {}）", link.name, link.index)
    }
}
