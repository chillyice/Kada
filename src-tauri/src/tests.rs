// 壳层单元测试（T-01：从 lib.rs 搬出，内容与顺序原样保留）。

use super::*;

use std::collections::BTreeSet;

use kada_core::Modifier;

/// `ui/src/options.ts` 的键表是 `kada-core` 的手抄副本（改键下拉 / 组合键录入显示名），
/// 一旦 Rust 新增 `Key` 而前端漏了，下拉会静默空白、保存时可能写空值（见规划 7.4-㉘）。
/// 这里按字面量解析前端数组 / 对象，跟 [`Key::ALL`]（与 `key_name` 同源生成）逐一比对。
///
/// 前端这两处必须是**显式字面量**（不能 spread / 循环拼），否则解析不到——这是有意的约束。
#[test]
fn frontend_key_table_matches_core() {
    // 拆包后键表跟着选项表去了 options.ts；录入映射（CODE_TABLE）跟着录入模块去了 input.ts。
    let read_src = |name: &str| {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/");
        std::fs::read_to_string(format!("{dir}{name}")).expect("读取前端源码失败")
    };
    let input = read_src("input.ts");
    let names: std::collections::BTreeSet<&str> = Key::ALL.iter().map(|k| key_name(*k)).collect();

    // ① 改键下拉的全键表：必须与 core 完全一致（不多不少）。
    let options = quoted_literals(region(&read_src("options.ts"), "const KEY_OPTIONS = ["));
    let options: std::collections::BTreeSet<&str> = options.iter().map(String::as_str).collect();
    let missing: Vec<&&str> = names.difference(&options).collect();
    let extra: Vec<&&str> = options.difference(&names).collect();
    assert!(
        missing.is_empty(),
        "KEY_OPTIONS 缺少 core 里的键：{missing:?}"
    );
    assert!(
        extra.is_empty(),
        "KEY_OPTIONS 有 core 里没有的键（拼写错 / 已删除）：{extra:?}"
    );

    // ② 录入时 KeyboardEvent.code → 键名的映射：每个显示名都必须是合法键名。
    for table in ["const CODE_TABLE", "const CODE_PUNCT"] {
        for lit in quoted_literals(region(&input, table)) {
            assert!(
                names.contains(lit.as_str()),
                "{table} 里的「{lit}」不是合法键名"
            );
        }
    }
}

/// 卡带市场（7.2-㉕）：索引 / 卡带的地址只收 http(s)。设置里那个字符串不该变成读本地
/// 文件的口子（`file://`、`\\?\C:\...` 之类一律拒在下载之前）。
#[test]
fn snippet_download_only_accepts_http_urls() {
    for bad in [
        "file:///C:/Windows/System32/config/SAM",
        r"\\?\C:\secrets.json",
        "ftp://example.com/index.json",
        "",
    ] {
        let err = tauri::async_runtime::block_on(http_get_text(bad)).unwrap_err();
        assert!(err.contains("不是 http"), "{bad} 应被拒：{err}");
    }
}

/// 「设备是」下拉（7.1-㊷ 的收尾）：本机输入设备 → 选项，值必须是**稳定片段**（实例号
/// 换 USB 口就变，存进去下次就不命中）；同型号两台压出同一个片段时退回完整路径，否则选
/// 哪台都命中两台。键盘 / 鼠标 / 触摸板 / 数位板都走这一套（7.1-㊷ 余项）。
#[test]
fn device_choices_prefers_stable_fragment_and_dedupes() {
    use kada_core::{DeviceKind, InputDevice};
    let hid =
        r"\\?\HID#VID_046D&PID_C52B&MI_00#7&b3c8a2e&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    let acpi = r"\\?\ACPI#MSI0007#4&1cc9388&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    let mk = |name: &str, kind| InputDevice {
        name: name.to_string(),
        kind,
    };
    let list = device_choices(vec![
        mk(hid, DeviceKind::Keyboard),
        mk(acpi, DeviceKind::Keyboard),
    ]);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "ACPI#MSI0007");
    assert_eq!(list[1].id, "VID_046D&PID_C52B");
    assert_eq!(list[1].detail, hid, "完整路径留在 detail 里供手写通配时抄");
    assert_eq!(list[1].kind, "keyboard");
    // 存的片段要真能命中原路径（否则选了也不生效）。`Condition` 随 automation 门控。
    #[cfg(feature = "automation")]
    assert!(kada_core::Condition::DeviceIs {
        id: list[1].id.clone()
    }
    .matches(&kada_core::Vars::new(), None, Some(hid)));

    // 同型号两台：片段相同 → 都退回完整路径（去掉接口类 GUID），仍互不相同。
    let twin =
        r"\\?\HID#VID_046D&PID_C52B&MI_00#7&22cd7391&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    let list = device_choices(vec![
        mk(hid, DeviceKind::Keyboard),
        mk(twin, DeviceKind::Keyboard),
    ]);
    assert_eq!(list.len(), 2);
    assert_ne!(list[0].id, list[1].id);
    assert!(
        list.iter().all(|d| !d.id.contains("884b96c3")),
        "接口类 GUID 不该写进配置"
    );

    // 同一块触摸板的两个口（触摸口 + 鼠标口）：片段相同但类别不同——只留更具体的
    // 「触摸板」，别在下拉里多出一条同名同值的「鼠标」。
    let pad_touch =
        r"\\?\HID#VID_04F3&PID_31B9&MI_00&Col01#7&aa&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
    let pad_mouse =
        r"\\?\HID#VID_04F3&PID_31B9&MI_00&Col02#7&aa&0&0000#{378a4e42-3a1e-4c5c-bd8b-1b1d0f0d9c39}";
    let list = device_choices(vec![
        mk(pad_touch, DeviceKind::Touchpad),
        mk(pad_mouse, DeviceKind::Mouse),
    ]);
    assert_eq!(list.len(), 1, "同一块硬件的两个口只留一条");
    assert_eq!(list[0].kind, "touchpad");
    assert_eq!(list[0].id, "VID_04F3&PID_31B9");

    // 罗技接收器：键盘与鼠标共用同一个 VID/PID，但**不是同一块硬件**（接口号不同）。
    // 这时不能靠片段相等去合并，否则键盘会从下拉里消失；两台都退回完整路径。
    let rcvr_kbd =
        r"\\?\HID#VID_046D&PID_C52B&MI_01&Col01#7&aa&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    let rcvr_mouse =
        r"\\?\HID#VID_046D&PID_C52B&MI_02&Col01#7&aa&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    let list = device_choices(vec![
        mk(rcvr_kbd, DeviceKind::Keyboard),
        mk(rcvr_mouse, DeviceKind::Mouse),
    ]);
    assert_eq!(list.len(), 2, "键盘与鼠标是两台设备，都要在");
    let kinds: Vec<&str> = list.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, vec!["keyboard", "mouse"]);
    assert!(
        list.iter().all(|d| d.id.contains("MI_0")),
        "同片段不同硬件 → 退回完整路径"
    );

    // 分组顺序：键盘 → 鼠标 → 数位板。
    let list = device_choices(vec![
        mk(
            r"\\?\HID#VID_056A&PID_0374&MI_02&Col01#7&cc&0&0000#{4d1e55b2}",
            DeviceKind::Tablet,
        ),
        mk("VID_046D&PID_C52B mouse", DeviceKind::Mouse),
        mk(acpi, DeviceKind::Keyboard),
    ]);
    let kinds: Vec<&str> = list.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, vec!["keyboard", "mouse", "tablet"]);
}

/// 卡带市场（7.2-㉕）端到端：真起一个本地 HTTP 服务，把仓库里的 `snippets/` 当托管端，
/// 走一遍「下载 → 解析索引 → 下载卡带 → 清洗 → 预览」——顺带钉住「仓库里那几份卡带
/// 文件本身就是合法的 `Config`」（字段名写错的话 serde 会静默忽略，装出来是空壳）。
#[test]
fn snippet_market_serves_repo_snippets_over_http() {
    let dir = format!("{}/../snippets", env!("CARGO_MANIFEST_DIR"));
    let (base, stop) = serve_dir(&dir);

    let index =
        tauri::async_runtime::block_on(http_get_text(&format!("{base}/index.json"))).unwrap();
    let entries = parse_snippet_index(&index).unwrap();
    assert!(!entries.is_empty(), "仓库里的索引没有条目");

    // 索引里的 url 指向 GitHub raw（上线才是那个地址），这里换成同一个本地服务：逐条下载，
    // 钉住「仓库里每份卡带都是合法且非空的 `Config`」（字段名写错 serde 会静默忽略，
    // 装出来是个空壳，而用户只会觉得「卡带市场是坏的」）。
    for e in &entries {
        let file = e.url.rsplit('/').next().unwrap();
        let got = tauri::async_runtime::block_on(fetch_snippet(format!("{base}/{file}")))
            .unwrap_or_else(|err| panic!("卡带 {file} 下载/解析失败：{err}"));
        let n =
            got.preview.shortcuts.len() + got.preview.remaps.len() + got.preview.expansions.len();
        assert!(n > 0, "卡带 {file} 清洗后一条条目都不剩");
    }

    let fetched =
        tauri::async_runtime::block_on(fetch_snippet(format!("{base}/caps-esc-ctrl.json")))
            .unwrap();
    let remap = &fetched.preview.remaps[0];
    assert_eq!(remap.from, "CapsLock");
    assert_eq!(remap.tap.as_deref(), Some("Escape"));
    assert_eq!(remap.hold.as_deref(), Some("Ctrl"));
    assert!(fetched.preview.shortcuts.is_empty());

    // 带层的卡带：被引用的层必须一起带过来（否则装进去层引用被清成基础层 = 静默改行为）。
    let vim =
        tauri::async_runtime::block_on(fetch_snippet(format!("{base}/vim-keys.json"))).unwrap();
    assert_eq!(vim.preview.remaps.len(), 6);
    assert!(!fetched.text.is_empty() && !vim.text.is_empty());
    stop.store(true, Ordering::Relaxed);
}

/// 内置示例卡带（「载入示例」与卡带市场同一批 JSON）：每份都能独立解析成非空 `Config`，
/// 合进示例配置后不制造冲突、清洗不丢条目（否则「载入示例」要么报错要么装出半截）。
#[test]
fn bundled_example_snippets_are_valid_configs() {
    let mut texts: Vec<&str> = BASIC_EXAMPLE_SNIPPETS.to_vec();
    #[cfg(feature = "automation")]
    texts.extend_from_slice(AUTOMATION_EXAMPLE_SNIPPETS);
    for text in texts {
        let part: Config =
            serde_json::from_str(text).unwrap_or_else(|e| panic!("内置示例卡带解析失败：{e}"));
        let n = part.shortcuts.len() + part.remaps.len() + part.expansions.len();
        assert!(n > 0, "内置示例卡带一条条目也没有");
        let (clean, notes) = sanitize_config(&part);
        assert!(notes.is_empty(), "内置示例卡带清洗报了问题：{notes:?}");
        assert_eq!(
            clean.shortcuts.len(),
            part.shortcuts.len(),
            "内置示例条目被清洗掉"
        );
    }
    let cfg = bundled_examples().expect("内置示例合并失败");
    assert!(
        cfg.shortcuts.len() > kada_core::sample::sample_config().shortcuts.len(),
        "内置示例卡带没贡献任何快捷键"
    );
    let conflicts = kada_core::detect_conflicts(&cfg, &kada_core::PlatformCaps::default());
    assert!(conflicts.is_empty(), "内置示例自带冲突：{conflicts:?}");
}

/// 卡带发布：POST 的 Content-Type / body 与响应解析。发布端点固定为官方地址
/// （`http_post_json` 不接受前端传 URL），这里只验传输层与机审判级。
#[test]
fn snippet_publish_posts_json_and_scans() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let head = String::from_utf8_lossy(&buf).to_string();
        let len = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        let already = buf.len() - (head.find("\r\n\r\n").unwrap_or(0) + 4);
        let mut body = buf[buf.len() - already..].to_vec();
        while body.len() < len {
            let n = s.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        let _ = tx.send(format!("{head}\n---\n{}", String::from_utf8_lossy(&body)));
        let resp = br#"{"code":0,"message":"ok","data":{"id":7,"status":"PENDING","level":2}}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            resp.len()
        );
        let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(resp));
    });

    let text =
        tauri::async_runtime::block_on(http_post_json(&base, r#"{"name":"测试","content":"{}"}"#))
            .unwrap();
    let seen = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(seen
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    assert!(seen.contains(r#"{"name":"测试","content":"{}"}"#));
    let env: PublishEnvelope = serde_json::from_str(&text).unwrap();
    assert_eq!(env.code, 0);
    assert_eq!(env.data.unwrap().level, 2);

    let scan = scan_snippet(r#"{"shortcuts":[{"triggers":["F1"],"actions":[{"type":"command","shell":"cmd","command":"whoami"}]}]}"#).unwrap();
    assert_eq!(scan.level, 3);
}

/// 社区索引合并：社区在前、同一 URL 只留一份（社区卡若与官方同 URL，以官方那条为准）。
#[test]
fn community_index_merges_before_official_without_duplicates() {
    let entry = |name: &str, url: &str, level: Option<u8>| IndexEntry {
        name: name.into(),
        url: url.into(),
        description: String::new(),
        author: String::new(),
        tags: vec![],
        level,
    };
    let official = vec![
        entry("官方 A", "https://x/a.json", None),
        entry("官方 B", "https://x/b.json", None),
    ];
    let community = vec![
        entry("社区 C", "https://x/c.json", Some(2)),
        entry("重复 B", "https://x/b.json", Some(3)),
    ];
    let merged = merge_index_entries(official, community);
    assert_eq!(
        merged.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["社区 C", "官方 A", "官方 B"]
    );
    assert_eq!(merged[0].level, Some(2));
}

/// 单线程的「静态文件服务」：回一个请求就够测试用（每条连接单独 spawn）。
fn serve_dir(dir: &str) -> (String, Arc<AtomicBool>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("端口绑定失败");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let dir = dir.to_string();
    let flag = stop.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if flag.load(Ordering::Relaxed) {
                return;
            }
            let Ok(mut s) = stream else { continue };
            use std::io::{BufRead, BufReader, Write};
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap_or_default();
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = std::fs::read(format!("{dir}{path}")).unwrap_or_default();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s
                .write_all(head.as_bytes())
                .and_then(|_| s.write_all(&body));
        }
    });
    (base, stop)
}

/// 取 `marker` 起到其后第一个 `];` / `};` 之间的源码片段。
fn region<'a>(src: &'a str, marker: &str) -> &'a str {
    let start = src
        .find(marker)
        .unwrap_or_else(|| panic!("找不到标记：{marker}"));
    let rest = &src[start..];
    let end = ["\n];", "\n};"]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()
        .unwrap_or_else(|| panic!("找不到结束标记：{marker}"));
    &rest[..end]
}

/// 抽出片段里所有双引号字面量的内容（处理 `\\` 与 `\"` 两种转义）。
fn quoted_literals(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let (mut j, mut val) = (i + 1, String::new());
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 1;
                    val.push(bytes[j] as char);
                } else {
                    val.push(bytes[j] as char);
                }
                j += 1;
            }
            out.push(val);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn external_link_filter_accepts_only_http() {
    // 界面外链（帮助页 GitHub）只把 http(s) 丢给系统浏览器，其余新窗口请求一律不理。
    assert_eq!(
        external_link_to_open("https://github.com/chillyice/Kada"),
        Some("https://github.com/chillyice/Kada")
    );
    assert_eq!(
        external_link_to_open("http://example.com/x?y=1&z=2"),
        Some("http://example.com/x?y=1&z=2")
    );
    for other in [
        "about:blank",
        "data:text/html,hi",
        "file:///C:/",
        "javascript:alert(1)",
        "tauri://localhost/index.html",
    ] {
        assert_eq!(external_link_to_open(other), None, "{other}");
    }
}

#[test]
fn injects_keys_only_matches_key_injecting_actions() {
    // 只有 Text / Keys 会往目标程序注入按键：修饰键没松时等的是它们（见 fire）。
    let text = vec![Action::Text {
        text: "x".into(),
        mode: kada_core::TextMode::Input,
        masked: false,
        description: None,
    }];
    let keys = vec![Action::Keys {
        keys: vec!["Ctrl".into(), "C".into()],
        mode: kada_core::KeysMode::Tap,
        description: None,
    }];
    assert!(injects_keys(&text));
    assert!(injects_keys(&keys));

    // 后台发送走 PostMessage，不受物理修饰键影响：不算 injects_keys，否则会白等 300ms
    // 甚至因超时把整条链跳过。
    let send_to = vec![Action::SendTo {
        target: "notepad.exe".into(),
        payload: kada_core::SendPayload::Keys {
            keys: vec!["Ctrl".into(), "S".into()],
        },
        description: None,
    }];
    assert!(!injects_keys(&send_to));

    // 鼠标模拟也注入输入：物理修饰键没松时同样会变成 Ctrl+点击 / 拖拽，等它释放。
    let mouse = vec![Action::Mouse {
        op: kada_core::MouseOp::Click {
            button: kada_core::MouseButton::Left,
        },
        description: None,
    }];
    assert!(injects_keys(&mouse));

    // 「临时文本」走剪贴板粘贴（发 Ctrl+V），要等；只写 / 读剪贴板不注入，不用等。
    let temp = vec![Action::Clipboard {
        operation: kada_core::ClipboardOperation::TempStr { text: "x".into() },
        description: None,
    }];
    assert!(injects_keys(&temp));
    let set_clip = vec![Action::Clipboard {
        operation: kada_core::ClipboardOperation::SetClipboard { text: "x".into() },
        description: None,
    }];
    assert!(!injects_keys(&set_clip));

    // 算法动作只有勾了「直接注入结果」才发输入；只写变量不用等修饰键。
    let transform_inject = vec![Action::Transform {
        operation: kada_core::TransformOperation::Base64Encode,
        input: "x".into(),
        var: "out".into(),
        inject: true,
        description: None,
    }];
    assert!(injects_keys(&transform_inject));
    let transform_var = vec![Action::Transform {
        operation: kada_core::TransformOperation::Base64Encode,
        input: "x".into(),
        var: "out".into(),
        inject: false,
        description: None,
    }];
    assert!(!injects_keys(&transform_var));

    // 命令 / 暂停不受修饰键影响，不该白等 300ms。
    let plain = vec![
        Action::PauseMs {
            ms: 10,
            description: None,
        },
        #[cfg(feature = "automation")]
        Action::OpenUrl {
            url: "https://kada.test".into(),
            description: None,
        },
    ];
    assert!(!injects_keys(&plain));

    // If 分支里的注入动作也算数（两路并查）。
    #[cfg(feature = "automation")]
    {
        let branched = vec![Action::If {
            condition: kada_core::Condition::Exists {
                path: "C:\\".into(),
            },
            then: vec![],
            otherwise: keys,
            description: None,
        }];
        assert!(injects_keys(&branched));
        let empty_branch = vec![Action::If {
            condition: kada_core::Condition::Exists {
                path: "C:\\".into(),
            },
            then: vec![],
            otherwise: vec![],
            description: None,
        }];
        assert!(!injects_keys(&empty_branch));
    }

    // 切换虚拟桌面也注入组合键，同样要等物理修饰键松开。
    #[cfg(feature = "automation")]
    {
        let desktop = vec![Action::System {
            operation: kada_core::SystemOperation::SwitchDesktop {
                direction: kada_core::DesktopDirection::Next,
            },
            description: None,
        }];
        assert!(injects_keys(&desktop));

        // 文件类操作不注入，别为它们白等 300ms。
        let file = vec![Action::Os {
            operation: kada_core::OsOperation::Delete {
                path: "C:\\tmp\\a.txt".into(),
            },
            description: None,
        }];
        assert!(!injects_keys(&file));
    }
}

/// 每个动作报两次（开始 running / 结束最终态，CS-07）：壳层按 id upsert——
/// 同 id 第二次覆盖第一条；被 `MAX_RUN_STEPS` 裁掉的步骤找不到就当新的一条 push。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[test]
fn add_step_upserts_by_id() {
    let mk = |id: u64, status: &str, end: u64| ActionStep {
        id,
        parent: None,
        lane: 0,
        kind: "command".into(),
        title: "echo hi".into(),
        branch: None,
        detail: None,
        status: status.into(),
        start_ms: 0,
        end_ms: end,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: Some(0),
        detached: false,
        log_path: None,
    };
    let mut steps: Vec<ActionStep> = Vec::new();
    upsert_step(&mut steps, mk(1, "running", 0));
    assert_eq!(steps.len(), 1, "先报一条执行中");
    assert_eq!(steps[0].status, "running");
    // 同 id 的最终态覆盖，不留两条。
    upsert_step(&mut steps, mk(1, "ok", 120));
    assert_eq!(steps.len(), 1, "同 id 该覆盖而不是追加");
    assert_eq!(steps[0].status, "ok");
    assert_eq!(steps[0].end_ms, 120);
    // 失败覆盖：最终态说了算（error 步把记录标 error 的逻辑在 add_step 里）。
    upsert_step(&mut steps, mk(2, "running", 0));
    upsert_step(&mut steps, mk(2, "error", 300));
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].status, "error");
}

/// 假注入器：只记录发出的键，绝不真的注入（会打到跑测试的这台机器上）。
#[derive(Default)]
struct TestInject {
    log: Vec<String>,
}

impl engine::Inject for TestInject {
    fn down(&mut self, key: Key) {
        self.log.push(format!("down {}", key_name(key)));
    }
    fn up(&mut self, key: Key) {
        self.log.push(format!("up {}", key_name(key)));
    }
}

fn down(key: Key) -> Ev {
    Ev::Down {
        key,
        mods: BTreeSet::new(),
        repeat: false,
    }
}

fn up(key: Key) -> Ev {
    Ev::Up { key }
}

/// 走一遍钩子回调的决策链（与 `run()` 里的接线一致）：`engine.step` → `decide`。
fn decide_via_engine(
    engine: &mut Engine,
    inj: &mut TestInject,
    cfg: &Config,
    ev: &Ev,
) -> Option<Outcome> {
    let mut fire = |_a: Vec<Action>, _t: String, _n: String| {};
    let ev = engine.step(ev, cfg, &TriggerContext::empty(), inj, &mut fire)?;
    Some(decide(
        &ev,
        cfg,
        engine.active_layer(),
        &TriggerContext::empty(),
    ))
}

/// 同 [`decide_via_engine`]，但把触发的触发键收集下来（和弦/序列在 `step` 里就 fire 了）。
fn step_collecting_fire(
    engine: &mut Engine,
    inj: &mut TestInject,
    cfg: &Config,
    ev: &Ev,
    fired: &mut Vec<String>,
) -> Option<Ev> {
    let mut fire = |_a: Vec<Action>, t: String, _n: String| fired.push(t);
    engine.step(ev, cfg, &TriggerContext::empty(), inj, &mut fire)
}

/// 「长按进入层」的切层键（CapsLock → L1）+ 该层内一条快捷键（触发键由参数给出）。
fn cfg_with_layer(trigger: &str) -> Config {
    Config {
        layers: vec![kada_core::Layer {
            id: "L1".into(),
            name: "层1".into(),
            ..Default::default()
        }],
        remaps: vec![kada_core::Remap {
            from: "CapsLock".into(),
            hold_layer: Some("L1".into()),
            enabled: true,
            ..Default::default()
        }],
        shortcuts: vec![kada_core::ShortcutItem {
            layer: Some("L1".into()),
            triggers: vec![trigger.into()],
            actions: vec![Action::Text {
                text: "x".into(),
                mode: kada_core::TextMode::Input,
                masked: false,
                description: None,
            }],
            enabled: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn layer_key_short_press_replays_original_key() {
    // 只设了「长按进入层」、没设短按输出：短按必须回放原键，否则这个键就变哑了
    //（用户按一下 CapsLock 什么也不发生）。
    let cfg = cfg_with_layer("K");
    let mut engine = Engine::new();
    let mut inj = TestInject::default();

    assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
    decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
    assert_eq!(
        inj.log,
        vec!["down CapsLock", "up CapsLock"],
        "短按回放原键"
    );
    assert_eq!(engine.active_layer(), None, "短按不进层");
}

#[test]
fn layer_key_long_press_without_other_key_replays_original_key() {
    // 「一直按住切层键、期间没按别的键就松开」（层从没真正进过）：整个按键必须回放原键，
    // 否则这个键只是按久了一点就彻底没反应——用户看到的正是「长按切层键没反应」。
    let mut cfg = cfg_with_layer("K");
    cfg.remaps[0].tap_timeout_ms = 20;
    let mut engine = Engine::new();
    let mut inj = TestInject::default();

    assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
    std::thread::sleep(std::time::Duration::from_millis(30));
    decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
    assert_eq!(
        inj.log,
        vec!["down CapsLock", "up CapsLock"],
        "长按无输出时回放原键"
    );
    assert_eq!(engine.active_layer(), None, "没按别的键就不进层");
}

#[test]
fn user_real_config_layer_key_tab_with_chord() {
    // 复刻用户真实配置（2026-09-24 14:25）：`Tab` 长按进「和弦层」，层内一条快捷键带
    // `5&Y`（和弦）与 `Alt+Z`（普通组合）。事件流按真人操作补齐：Tab 按住（含自动重复）
    // → 5 → Y → 松开。和弦与组合都应在按住切层键期间命中。
    const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
    let cfg = Config {
        layers: vec![kada_core::Layer {
            id: LID.into(),
            name: "和弦层".into(),
            ..Default::default()
        }],
        remaps: vec![kada_core::Remap {
            from: "Tab".into(),
            to: String::new(),
            hold_layer: Some(LID.into()),
            tap_timeout_ms: 200,
            enabled: true,
            ..Default::default()
        }],
        shortcuts: vec![kada_core::ShortcutItem {
            layer: Some(LID.into()),
            triggers: vec!["5&Y".into(), "Alt+Z".into()],
            actions: vec![Action::Text {
                text: "pw".into(),
                mode: kada_core::TextMode::Input,
                masked: false,
                description: None,
            }],
            enabled: true,
            ..Default::default()
        }],
        ..Default::default()
    };

    // ① 和弦：按住 Tab（含自动重复）→ 5 → Y
    let mut engine = Engine::new();
    let mut inj = TestInject::default();
    let mut fired: Vec<String> = Vec::new();
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
    for _ in 0..5 {
        let repeat = Ev::Down {
            key: Key::Tab,
            mods: BTreeSet::new(),
            repeat: true,
        };
        step_collecting_fire(&mut engine, &mut inj, &cfg, &repeat, &mut fired);
    }
    assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
    assert_eq!(engine.active_layer(), Some(LID), "按住 Tab + 按 5 → 进层");
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
    assert_eq!(fired, vec!["5&Y"], "层内和弦应触发");

    // ② 普通组合：松开和弦成员后，按住 Tab 再按 Alt+Z（真人按 Alt 时事件带 mods={Alt}）
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Digit5), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Y), &mut fired);
    fired.clear();
    let alt_down = Ev::Down {
        key: Key::Alt,
        mods: BTreeSet::from([Modifier::Alt]),
        repeat: false,
    };
    step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_down, &mut fired);
    let z_down = Ev::Down {
        key: Key::Z,
        mods: BTreeSet::from([Modifier::Alt]),
        repeat: false,
    };
    let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &z_down, &mut fired);
    let layer = engine.active_layer().map(str::to_string);
    match decide(
        &out.expect("Alt+Z 应放行进 decide"),
        &cfg,
        layer.as_deref(),
        &TriggerContext::empty(),
    ) {
        Outcome::Shortcut { trigger, .. } => assert_eq!(trigger, "Alt+Z"),
        other => panic!(
            "层内 Alt+Z 应命中该层快捷键，实际 {:?}",
            matches!(other, Outcome::Pass)
        ),
    }

    // ③ 层内序列：按住 Tab 期间依次按 F9 J K 应触发
    let mut cfg2 = cfg.clone();
    cfg2.shortcuts[0].triggers = vec!["F9 J K".into()];
    let mut engine2 = Engine::new();
    let mut inj2 = TestInject::default();
    let mut fired2: Vec<String> = Vec::new();
    step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::Tab), &mut fired2);
    step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::F9), &mut fired2);
    assert_eq!(engine2.active_layer(), Some(LID), "按住 Tab + 按 F9 → 进层");
    step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::J), &mut fired2);
    step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::K), &mut fired2);
    assert_eq!(fired2, vec!["F9 J K"], "层内序列应触发");
}

#[test]
fn layer_key_released_before_chord_does_not_fire() {
    // 层是 momentary 的（hold_layer）：松开切层键后层立即失效，此时按层内和弦不触发（这是
    // 设计，不是 bug）。「长按切层、松开、再按层内和弦/序列」的用法要用切换式切层
    //（lock_layer，见 locked_layer_* 用例）。
    let cfg = cfg_with_layer("5&Y");
    let mut engine = Engine::new();
    let mut inj = TestInject::default();
    let mut fired: Vec<String> = Vec::new();

    step_collecting_fire(
        &mut engine,
        &mut inj,
        &cfg,
        &down(Key::CapsLock),
        &mut fired,
    );
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::CapsLock), &mut fired);
    assert_eq!(engine.active_layer(), None, "松开切层键即退层");
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
    assert!(
        fired.is_empty(),
        "层未激活时层内和弦不触发，实际：{fired:?}"
    );
}

#[test]
fn locked_layer_key_long_press_then_chord_sequence_combo() {
    // 复刻用户真实配置（2026-09-24）：`Tab` 改成「长按锁定层」（切换式）指向「和弦层」，
    // 层内一条快捷键带 `5&Y`（和弦）+ `F9 J K`（序列）+ `Alt+Z`（普通组合），基础层也有一条
    // `Alt+Z`。用户诉求：**长按切层键之后（松手）** 层内的和弦与序列要能触发——momentary
    // 做不到（层随松手消失，层内和弦还得一直按住切层键），切换式才用得起来。
    const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
    let cfg = Config {
        layers: vec![kada_core::Layer {
            id: LID.into(),
            name: "和弦层".into(),
            ..Default::default()
        }],
        remaps: vec![kada_core::Remap {
            from: "Tab".into(),
            to: String::new(),
            lock_layer: Some(LID.into()),
            tap_timeout_ms: 20, // 真实配置是 200；测试里压短，免得真等
            enabled: true,
            ..Default::default()
        }],
        shortcuts: vec![
            kada_core::ShortcutItem {
                name: Some("基础层 Alt+Z".into()),
                triggers: vec!["Alt+Z".into()],
                actions: vec![],
                enabled: true,
                ..Default::default()
            },
            kada_core::ShortcutItem {
                name: Some("输入密码".into()),
                layer: Some(LID.into()),
                triggers: vec!["5&Y".into(), "F9 J K".into(), "Alt+Z".into()],
                actions: vec![Action::Text {
                    text: "pw".into(),
                    mode: kada_core::TextMode::Input,
                    masked: false,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let mut engine = Engine::new();
    let mut inj = TestInject::default();
    let mut fired: Vec<String> = Vec::new();

    // ① 长按 Tab（期间没按别的键）再松开 → 切进「和弦层」并保持生效。
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
    std::thread::sleep(std::time::Duration::from_millis(30));
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Tab), &mut fired);
    assert_eq!(engine.active_layer(), Some(LID), "长按切层后层保持生效");
    assert!(inj.log.is_empty(), "切层键不回放");

    // ② 层内和弦：松手后直接按 5 与 Y。
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
    assert_eq!(fired, vec!["5&Y"], "长按切层后层内和弦应触发");

    // ③ 层内序列：F9 J K 依次按下。
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Digit5), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Y), &mut fired);
    fired.clear();
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::F9), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::J), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::K), &mut fired);
    assert_eq!(fired, vec!["F9 J K"], "长按切层后层内序列应触发");

    // ④ 层内普通组合：层条目优先于基础层同名条目。
    fired.clear();
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::F9), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::J), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::K), &mut fired);
    let alt_z = Ev::Down {
        key: Key::Z,
        mods: BTreeSet::from([Modifier::Alt]),
        repeat: false,
    };
    let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_z, &mut fired);
    let layer = engine.active_layer().map(str::to_string);
    match decide(
        &out.expect("Alt+Z 应放行进 decide"),
        &cfg,
        layer.as_deref(),
        &TriggerContext::empty(),
    ) {
        Outcome::Shortcut { name, .. } => assert_eq!(name, "输入密码", "层内 Alt+Z 应优先"),
        Outcome::Replace(_) => panic!("不该走改键"),
        Outcome::Pass => panic!("层内 Alt+Z 未命中"),
    }

    // ⑤ 再长按一次切层键 → 退出锁定层，Alt+Z 回到基础层条目。
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
    std::thread::sleep(std::time::Duration::from_millis(30));
    step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Tab), &mut fired);
    assert_eq!(engine.active_layer(), None, "再长按一次退出切层");
    let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_z, &mut fired);
    match decide(
        &out.expect("Alt+Z 应放行进 decide"),
        &cfg,
        None,
        &TriggerContext::empty(),
    ) {
        Outcome::Shortcut { name, .. } => assert_eq!(name, "基础层 Alt+Z"),
        Outcome::Replace(_) => panic!("不该走改键"),
        Outcome::Pass => panic!("基础层 Alt+Z 未命中"),
    }
}

#[test]
fn layer_key_long_press_activates_layer_shortcut() {
    // 长按切层键期间按下 K：K 应按「激活层优先」命中层内快捷键（而不是基础层兜底）。
    let cfg = cfg_with_layer("K");
    let mut engine = Engine::new();
    let mut inj = TestInject::default();

    assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
    assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
    let out = decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::K)).expect("K 应放行");
    assert_eq!(
        engine.active_layer(),
        Some("L1"),
        "按住切层键 + 按 K → 进层"
    );
    match out {
        Outcome::Shortcut { trigger, .. } => assert_eq!(trigger, "K"),
        _ => panic!("层内 K 应命中该层快捷键"),
    }

    // 松开切层键退回基础层，K 不再命中层条目。
    decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::K));
    decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
    assert_eq!(engine.active_layer(), None, "松开切层键退回基础层");
    let out = decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::K)).expect("K 应放行");
    assert!(
        matches!(out, Outcome::Pass),
        "基础层下 K 不该命中层内快捷键"
    );
}

#[test]
fn layer_chord_fires_while_layer_key_held() {
    // 复刻实际配法：层「和弦层」里放一条和弦快捷键（5&Y），用 CapsLock 长按进层
    //（CapsLock 只设了「长按进入层」）。按住切层键 + 同时按 5 与 Y → 该层和弦应触发。
    let cfg = cfg_with_layer("5&Y");
    let mut fired: Vec<String> = Vec::new();
    let mut inj = TestInject::default();
    let mut engine = Engine::new();

    step_collecting_fire(
        &mut engine,
        &mut inj,
        &cfg,
        &down(Key::CapsLock),
        &mut fired,
    );
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
    assert_eq!(engine.active_layer(), Some("L1"), "按住切层键即进层");
    assert_eq!(fired, vec!["5&Y"], "层内和弦应触发");

    // 不进层时同一个和弦不该触发（层语义：条目只在该层激活时生效）。
    let mut fired2: Vec<String> = Vec::new();
    let mut inj2 = TestInject::default();
    let mut engine2 = Engine::new();
    step_collecting_fire(
        &mut engine2,
        &mut inj2,
        &cfg,
        &down(Key::Digit5),
        &mut fired2,
    );
    step_collecting_fire(&mut engine2, &mut inj2, &cfg, &down(Key::Y), &mut fired2);
    assert!(
        fired2.is_empty(),
        "基础层下不应命中层内和弦，实际：{fired2:?}"
    );
}

#[cfg(feature = "automation")]
#[test]
fn when_gate_routes_same_shortcut_by_frontmost() {
    use kada_core::{Condition, FrontmostContext, TriggerContext};
    let mk = |name: &str, when: Option<Condition>| kada_core::ShortcutItem {
        name: Some(name.into()),
        triggers: vec!["Ctrl+K".into()],
        actions: vec![],
        when,
        enabled: true,
        ..Default::default()
    };
    // 同触发键 Ctrl+K：一条只在 Chrome 生效，另一条无条件兜底。
    let cfg = Config {
        shortcuts: vec![
            mk(
                "Chrome",
                Some(Condition::FrontmostApp {
                    app: "chrome".into(),
                }),
            ),
            mk("兜底", None),
        ],
        ..Default::default()
    };
    let ev = Ev::Down {
        key: Key::K,
        mods: BTreeSet::from([Modifier::Ctrl]),
        repeat: false,
    };

    let chrome = FrontmostContext {
        process_name: "chrome.exe".into(),
        window_title: String::new(),
    };
    match decide(&ev, &cfg, None, &TriggerContext::fixed(Some(&chrome), None)) {
        Outcome::Shortcut { name, .. } => assert_eq!(name, "Chrome"),
        other => panic!(
            "应命中 Chrome 条目，实际 Pass={}",
            matches!(other, Outcome::Pass)
        ),
    }

    // 别的前台：条件条目跳过，落到兜底条目。
    let code = FrontmostContext {
        process_name: "Code.exe".into(),
        window_title: String::new(),
    };
    match decide(&ev, &cfg, None, &TriggerContext::fixed(Some(&code), None)) {
        Outcome::Shortcut { name, .. } => assert_eq!(name, "兜底"),
        other => panic!(
            "应落到兜底条目，实际 Pass={}",
            matches!(other, Outcome::Pass)
        ),
    }

    // 无前台上下文（Linux）：条件条目恒不成立，仍落到兜底。
    match decide(&ev, &cfg, None, &TriggerContext::empty()) {
        Outcome::Shortcut { name, .. } => assert_eq!(name, "兜底"),
        _ => panic!("无前台下应落到兜底条目"),
    }
}

#[test]
fn user_real_config_backtick_layer_chord_u_amp_y() {
    // 复刻用户 2026-09-30 真实配置：` (Backquote) 长按进「和弦层」，层内条目带
    // "U&Y"（和弦）+ "Alt+Z"；基础层另有一条 Alt+Z。事件流按真人操作：` 按住（含
    // 自动重复）→ U → Y。
    const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
    let cfg = Config {
        layers: vec![kada_core::Layer {
            id: LID.into(),
            name: "和弦层".into(),
            ..Default::default()
        }],
        remaps: vec![kada_core::Remap {
            from: "`".into(),
            to: String::new(),
            hold_layer: Some(LID.into()),
            tap_timeout_ms: 200,
            enabled: true,
            ..Default::default()
        }],
        shortcuts: vec![
            kada_core::ShortcutItem {
                name: Some("生成zcode".into()),
                triggers: vec!["Alt+Z".into()],
                actions: vec![],
                enabled: true,
                ..Default::default()
            },
            kada_core::ShortcutItem {
                name: Some("输入密码".into()),
                layer: Some(LID.into()),
                triggers: vec!["Alt+Z".into(), "U&Y".into()],
                actions: vec![],
                enabled: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let mut engine = Engine::new();
    let mut inj = TestInject::default();
    let mut fired: Vec<String> = Vec::new();

    step_collecting_fire(
        &mut engine,
        &mut inj,
        &cfg,
        &down(Key::Backquote),
        &mut fired,
    );
    // 按住 ` 期间系统会发自动重复的 Down。
    for _ in 0..5 {
        let repeat = Ev::Down {
            key: Key::Backquote,
            mods: BTreeSet::new(),
            repeat: true,
        };
        step_collecting_fire(&mut engine, &mut inj, &cfg, &repeat, &mut fired);
    }
    assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::U), &mut fired);
    assert_eq!(engine.active_layer(), Some(LID), "按住 ` + 按 U → 进层");
    step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
    assert_eq!(fired, vec!["U&Y"], "按住切层键时 U&Y 应触发");
}

#[test]
fn recorder_transcribes_events() {
    let down = |key: Key| Ev::Down {
        key,
        mods: BTreeSet::new(),
        repeat: false,
    };
    let up = |key: Key| Ev::Up { key };

    let mut r = Recorder::new();
    r.push(&down(Key::K));
    r.push(&up(Key::K));
    r.push(&down(Key::Control));
    r.push(&down(Key::C));
    r.push(&up(Key::C));
    r.push(&up(Key::Control));
    let actions = r.finish();

    // K 单键成一条组合键，Ctrl+C 合并成一条组合键（间隔若 ≥20ms 会有 PauseMs，忽略之）
    let combos: Vec<&Vec<String>> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Keys { keys, .. } => Some(keys),
            _ => None,
        })
        .collect();
    assert!(combos.iter().any(|k| *k == &vec!["K".to_string()]));
    assert!(combos
        .iter()
        .any(|k| *k == &vec!["Ctrl".to_string(), "C".to_string()]));

    // 重复 down 折叠：按住 Ctrl 只产生一次组合键
    let mut r = Recorder::new();
    r.push(&down(Key::Control));
    r.push(&Ev::Down {
        key: Key::Control,
        mods: BTreeSet::new(),
        repeat: true,
    });
    r.push(&up(Key::Control));
    let ctrl = r
        .finish()
        .iter()
        .filter(|a| matches!(a, Action::Keys { keys, .. } if keys == &vec!["Ctrl".to_string()]))
        .count();
    assert_eq!(ctrl, 1, "重复 down 必须折叠");
}

#[test]
fn mouse_recorder_transcribes_events() {
    use kada_hook::{MouseRecord, MouseRecordKind};

    let ev = |kind: MouseRecordKind, move_dx: i32, move_dy: i32| MouseRecord {
        kind,
        move_dx,
        move_dy,
    };
    let mut r = MouseRecorder::default();
    r.push(&ev(
        MouseRecordKind::Click(kada_core::MouseButton::Left),
        120,
        -40,
    ));
    r.push(&ev(MouseRecordKind::Scroll { dx: 0, dy: -3 }, 0, 0));
    r.push(&ev(
        MouseRecordKind::Click(kada_core::MouseButton::Back),
        0,
        60,
    ));

    let ops: Vec<kada_core::MouseOp> = r
        .into_actions()
        .into_iter()
        .map(|a| match a {
            Action::Mouse { op, .. } => op,
            other => panic!("鼠标录制只应产出 Action::Mouse，得到 {other:?}"),
        })
        .collect();

    // 位移非 0 先补 Move；若位移到下一个触发之间没挪动则不补（滚轮那条）。
    assert_eq!(
        ops,
        vec![
            kada_core::MouseOp::Move { dx: 120, dy: -40 },
            kada_core::MouseOp::Click {
                button: kada_core::MouseButton::Left
            },
            kada_core::MouseOp::Scroll { dx: 0, dy: -3 },
            kada_core::MouseOp::Move { dx: 0, dy: 60 },
            kada_core::MouseOp::Click {
                button: kada_core::MouseButton::Back
            },
        ]
    );
}

/// 只有「快速唤醒键」一项不同的配置。
fn wake_cfg(wake_key: Option<&str>) -> RwLock<Config> {
    let mut cfg = Config::default();
    cfg.settings.wake_key = wake_key.map(str::to_string);
    RwLock::new(cfg)
}

#[test]
fn wake_double_tap_needs_two_taps_in_window() {
    // 判定已与「建窗口」解耦（见 wake_double_tap 的注释），所以这层时序规则能单测。
    let cfg = wake_cfg(Some("Alt"));
    let mut last = None;
    assert!(
        !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
        "第一击只是单击"
    );
    assert!(
        wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
        "窗口内的第二击命中"
    );
    assert!(
        !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
        "计完归零，第三击重新算单击"
    );
    assert!(
        wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
        "第四击又凑成一次双击"
    );
}

#[test]
fn wake_double_tap_window_expires_and_other_keys_reset() {
    let cfg = wake_cfg(Some("Alt"));

    // 上一次单击已落在判定窗口之外：这一击只算新的单击，不该唤窗。
    let mut last = Some(Instant::now() - Duration::from_millis(WAKE_DOUBLE_TAP_MS + 50));
    assert!(
        !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
        "超出窗口不算双击"
    );
    assert!(last.is_some(), "超时后要重新计时（这一击仍是单击）");

    // 中间按了别的键 → 上一次单击作废（那次多半是 Alt+Tab 这类组合的一部分）。
    let mut last = None;
    assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)));
    assert!(!wake_double_tap(&cfg, &mut last, &down(Key::K)));
    assert!(last.is_none(), "按其它键要清掉单击计数");
    assert!(
        !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
        "清空后这一击是新单击"
    );

    // 自动重复的按下不是独立一击：长按唤醒键不该被当成连击而唤窗。
    let mut last = None;
    let repeated = Ev::Down {
        key: Key::Alt,
        mods: BTreeSet::new(),
        repeat: true,
    };
    assert!(
        !wake_double_tap(&cfg, &mut last, &repeated),
        "自动重复不算一击"
    );
    assert!(last.is_none(), "自动重复也不该开始计时");
}

/// 复制快捷菜单（EXT-13）的双击判定：修饰键必须恰为 Ctrl、忽略 repeat 与抬起事件、
/// 超窗只重新计时。菜单只在真机上弹，这条纯函数是唯一能自动验的部分。
#[test]
fn double_copy_requires_exact_ctrl_c_and_fresh_window() {
    let menu = ClipboardMenu {
        enabled: true,
        double_copy_ms: 400,
        items: Vec::new(),
    };
    let ctrl_c = Ev::Down {
        key: Key::C,
        mods: BTreeSet::from([Modifier::Ctrl]),
        repeat: false,
    };
    let mut last = None;
    assert!(
        !double_copy_hit(&menu, &ctrl_c, &mut last),
        "第一击只是单击"
    );
    assert!(
        double_copy_hit(&menu, &ctrl_c, &mut last),
        "窗口内第二击命中"
    );
    assert!(
        !double_copy_hit(&menu, &ctrl_c, &mut last),
        "命中后归零，第三击重新算单击"
    );

    // 修饰键不精确 / repeat / 别的键 / 抬起事件都不算，也不该开始计时。
    for ev in [
        Ev::Down {
            key: Key::C,
            mods: BTreeSet::from([Modifier::Ctrl, Modifier::Shift]),
            repeat: false,
        },
        Ev::Down {
            key: Key::C,
            mods: BTreeSet::new(),
            repeat: false,
        },
        Ev::Down {
            key: Key::C,
            mods: BTreeSet::from([Modifier::Ctrl]),
            repeat: true,
        },
        Ev::Down {
            key: Key::V,
            mods: BTreeSet::from([Modifier::Ctrl]),
            repeat: false,
        },
        Ev::Up { key: Key::C },
    ] {
        let mut last = None;
        assert!(!double_copy_hit(&menu, &ev, &mut last), "不该命中: {ev:?}");
        assert!(last.is_none(), "不该开始计时: {ev:?}");
    }

    // 超出窗口：这一击只算新的单击。
    let mut last = Some(Instant::now() - Duration::from_millis(menu.double_copy_ms + 50));
    assert!(!double_copy_hit(&menu, &ctrl_c, &mut last), "超窗不算双击");
    assert!(last.is_some(), "超时后要重新计时");
}

#[test]
fn wake_disabled_or_unparsable_key_never_wakes() {
    // 未设唤醒键 = 快速唤醒关闭：按多少次都不唤窗。
    let off = wake_cfg(None);
    let mut last = None;
    assert!(!wake_double_tap(&off, &mut last, &down(Key::Alt)));
    assert!(!wake_double_tap(&off, &mut last, &down(Key::Alt)));

    // 配置里是解析不了的键名：当作关闭，既不 panic 也不唤窗（手工改配置能改出这种值）。
    let bad = wake_cfg(Some("没这个键"));
    let mut last = None;
    assert!(!wake_double_tap(&bad, &mut last, &down(Key::Alt)));
    assert!(!wake_double_tap(&bad, &mut last, &down(Key::Alt)));
}

// ---- 前台切换判定（输入状态复位的触发条件，见规划 7.2-④） ----

#[test]
fn focus_tracker_needs_two_sightings_of_new_window() {
    let mut t = FocusTracker::new(Some(1));
    assert!(!t.observe(Some(1)), "同一窗口不算切换");
    assert!(!t.observe(Some(2)), "刚看到的新窗口先记下，等下一轮确认");
    assert!(t.observe(Some(2)), "连续两轮都是它 → 切换成立");
    assert!(!t.observe(Some(2)), "确认之后不再重复报告");
}

#[test]
fn focus_tracker_ignores_transient_window_flicker() {
    // 触发气泡 / 右键菜单这类瞬态窗口只出现一轮：不该被当成「用户换了窗口」，
    // 否则用户手上正凑的和弦会被白复位掉。
    let mut t = FocusTracker::new(Some(1));
    assert!(!t.observe(Some(9)), "瞬态窗口：只看到一轮");
    assert!(!t.observe(Some(1)), "又回到原窗口 → 什么都不发生");
    assert!(!t.observe(Some(1)));
    // 真的换了：连续两轮都停在 2。
    assert!(!t.observe(Some(2)));
    assert!(t.observe(Some(2)));
}

#[test]
fn focus_tracker_treats_missing_handle_as_no_change() {
    // 锁屏 / 桌面切换瞬间会读不到前台窗口：不作数，也不该把候选位冲掉
    //（否则锁屏读空后回到原窗口会被误判成切换）。
    let mut t = FocusTracker::new(Some(1));
    assert!(!t.observe(None));
    assert!(!t.observe(None));
    assert!(!t.observe(Some(1)), "读空之后回到原窗口：没换过");
}

// ---- 输入状态指示载荷（托盘提示 / 悬浮指示，见规划 7.3-⑬） ----

/// 层 + 三类修饰键 → 载荷：层名要换成显示名，修饰键要按形态分开、摘要直接可读。
#[test]
fn status_payload_maps_layer_name_and_modifier_kinds() {
    let cfg = Config {
        layers: vec![kada_core::Layer {
            id: "L1".into(),
            name: "游戏层".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let st = EngineStatus {
        layer: Some("L1".into()),
        layer_locked: true,
        hold: vec![Key::Control],
        sticky: vec![],
        oneshot: vec![Key::Alt],
    };
    let p = build_status(&st, &cfg);
    assert_eq!(p.layer.as_deref(), Some("游戏层"), "层 id 要换成显示名");
    assert!(p.locked);
    let kinds: Vec<(&str, &str)> = p.mods.iter().map(|m| (m.kind, m.key.as_str())).collect();
    assert_eq!(
        kinds,
        vec![("hold", "Ctrl"), ("oneshot", "Alt")],
        "形态不能混为一谈"
    );
    assert_eq!(p.summary, "层：游戏层（锁定） · 按住 Ctrl · 单次 Alt");
    assert!(!p.idle());
}

/// 层没起名字就显示 id；id 已不在配置里（刚被外部改动删掉）也照原样显示，不能变成空白。
#[test]
fn status_payload_falls_back_to_layer_id() {
    let cfg = Config::default();
    let st = EngineStatus {
        layer: Some("ghost".into()),
        ..Default::default()
    };
    let p = build_status(&st, &cfg);
    assert_eq!(p.layer.as_deref(), Some("ghost"));
    assert_eq!(p.summary, "层：ghost（按住）");
    assert!(!p.locked, "没标锁定就按「按住」说");
}

/// 非修饰键也能单次 / 粘滞 / 长按（规划 7.3-⑭）：指示要按它的键名如实显示，
/// 而不是「只认修饰键」时那样整条消失。
#[test]
fn status_payload_shows_plain_keys() {
    let st = EngineStatus {
        hold: vec![Key::Enter],
        sticky: vec![Key::MouseBack],
        oneshot: vec![Key::Alt],
        ..Default::default()
    };
    let p = build_status(&st, &Config::default());
    let kinds: Vec<(&str, &str)> = p.mods.iter().map(|m| (m.kind, m.key.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            ("hold", "Enter"),
            ("sticky", "MouseBack"),
            ("oneshot", "Alt")
        ]
    );
    assert_eq!(p.summary, "按住 Enter · 粘滞 MouseBack · 单次 Alt");
}

/// 空状态：指示要判成 idle（据此隐藏悬浮窗、托盘提示回到应用名），且序列化里不带 `layer`。
#[test]
fn status_payload_is_idle_when_nothing_is_active() {
    let p = build_status(&EngineStatus::default(), &Config::default());
    assert!(p.idle());
    assert_eq!(p.summary, "");
    assert!(!serde_json::to_string(&p).unwrap().contains("layer"));
}

// ---- 提示框悬停让位（见 `spawn_hints_hover_watch`） ----

/// 进框即让位、停留够久才恢复、出框后下次照旧让位（`Out ↔ Passing → Settled`）。
#[test]
fn hints_hover_yields_then_settles_and_rearms() {
    let rect = (100, 200, 300, 400);
    // 光标还没进框：可交互。
    assert!(!point_in_rect((90.0, 200.0), rect));
    assert!(!point_in_rect((400.0, 200.0), rect), "右边界算外面");
    assert!(!point_in_rect((100.0, 600.0), rect), "下边界算外面");
    assert!(point_in_rect((100.0, 200.0), rect), "左上角算里面");
    assert!(point_in_rect((399.9, 599.9), rect));

    // 进框：让位（穿透）。
    let cur = hints_hover_step(HintsHover::Out, true, 0);
    assert_eq!(cur, HintsHover::Passing);
    // 让位期间挪出去：回可交互。
    assert_eq!(hints_hover_step(cur, false, 500), HintsHover::Out);
    // 让位期间还没停够：不恢复。
    assert_eq!(
        hints_hover_step(HintsHover::Passing, true, HINTS_HOVER_DWELL_MS - 1),
        HintsHover::Passing
    );
    // 停够了：恢复可交互，且一直保持到挪出边界。
    let settled = hints_hover_step(HintsHover::Passing, true, HINTS_HOVER_DWELL_MS);
    assert_eq!(settled, HintsHover::Settled);
    assert_eq!(
        hints_hover_step(settled, true, HINTS_HOVER_DWELL_MS + 60_000),
        HintsHover::Settled
    );
    // 挪出去之后再进来：又让位（否则「一直悬着」就再也点不到框了）。
    assert_eq!(hints_hover_step(settled, false, 0), HintsHover::Out);
    assert_eq!(
        hints_hover_step(HintsHover::Out, true, 0),
        HintsHover::Passing
    );
}

/// 同步存档（规划 7.2-㉝）：`sync_file` 拼固定文件名、空白目录视为未配置；
/// `push_sync` 把整份配置原子写到该目录（写出去的是能解析回来的同一份配置）。
#[test]
fn sync_archive_writes_config_to_dir() {
    let mut cfg = Config::default();
    assert!(
        sync_file(&cfg).is_none(),
        "未配置同步目录时不该有任何同步文件"
    );
    cfg.settings.sync_dir = Some("   ".into());
    assert!(sync_file(&cfg).is_none(), "空白目录视为未配置");

    let dir = std::env::temp_dir().join(format!("kada_sync_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    cfg.settings.sync_dir = Some(dir.display().to_string());
    cfg.settings.action_timeout_ms = 1234;

    assert_eq!(sync_file(&cfg).unwrap(), dir.join(SYNC_FILE_NAME));
    push_sync(&cfg);

    let text = std::fs::read_to_string(dir.join(SYNC_FILE_NAME)).unwrap();
    let back: Config = serde_json::from_str(&text).unwrap();
    assert_eq!(
        back.settings.action_timeout_ms, 1234,
        "同步出去的是整份配置"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
