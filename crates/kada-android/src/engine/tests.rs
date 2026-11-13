//! 安卓判定器的单测（宿主机跑，不需要设备）：非破坏式吞键 + 与桌面同一套触发语义。

use std::collections::BTreeSet;

use kada_core::{Action, Config, Key, Modifier, ShortcutItem};

use super::{AndroidEngine, Rules};

fn url(text: &str) -> Action {
    Action::OpenUrl {
        url: text.into(),
        description: None,
    }
}

fn item(name: &str, triggers: &[&str], actions: Vec<Action>) -> ShortcutItem {
    ShortcutItem {
        name: Some(name.into()),
        triggers: triggers.iter().map(|t| t.to_string()).collect(),
        actions,
        enabled: true,
        ..ShortcutItem::default()
    }
}

fn setup(items: Vec<ShortcutItem>) -> (Config, Rules) {
    let cfg = Config {
        shortcuts: items,
        ..Config::default()
    };
    let rules = Rules::build(&cfg);
    (cfg, rules)
}

fn ms(list: &[Modifier]) -> BTreeSet<Modifier> {
    list.iter().copied().collect()
}

fn press(
    engine: &mut AndroidEngine,
    cfg: &Config,
    rules: &Rules,
    key: Key,
    mods: &[Modifier],
) -> super::Decision {
    engine.on_key(cfg, rules, key, true, false, ms(mods), None)
}

fn release(engine: &mut AndroidEngine, cfg: &Config, rules: &Rules, key: Key) -> super::Decision {
    engine.on_key(cfg, rules, key, false, false, BTreeSet::new(), None)
}

#[test]
fn single_key_swallows_down_and_up() {
    let (cfg, rules) = setup(vec![item("f8", &["F8"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    let down = press(&mut engine, &cfg, &rules, Key::F8, &[]);
    assert!(down.swallow, "命中的触发键必须吞掉");
    assert_eq!(down.fired.len(), 1);
    assert_eq!(down.fired[0].trigger, "F8");

    let up = release(&mut engine, &cfg, &rules, Key::F8);
    assert!(
        up.swallow,
        "按下被吞的键，抬起也要吞（否则 app 看到半截按键）"
    );
}

#[test]
fn combo_swallows_only_the_trigger_key() {
    let (cfg, rules) = setup(vec![item("k", &["Ctrl+Alt+K"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    // 修饰键先按下：非破坏式，一律放行（桌面版的「先吞后回放」在这里做不到）。
    let ctrl = press(&mut engine, &cfg, &rules, Key::Control, &[Modifier::Ctrl]);
    assert!(!ctrl.swallow);
    let alt = press(
        &mut engine,
        &cfg,
        &rules,
        Key::Alt,
        &[Modifier::Ctrl, Modifier::Alt],
    );
    assert!(!alt.swallow);

    let k = press(
        &mut engine,
        &cfg,
        &rules,
        Key::K,
        &[Modifier::Ctrl, Modifier::Alt],
    );
    assert!(k.swallow, "凑齐那一下要吞");
    assert_eq!(k.fired[0].name, "k");
}

#[test]
fn chord_passes_members_and_swallows_the_completing_key() {
    let (cfg, rules) = setup(vec![item("chord", &["F&J"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    let f = press(&mut engine, &cfg, &rules, Key::F, &[]);
    assert!(!f.swallow, "和弦成员先放行（吞错的键补不回来）");
    assert!(f.fired.is_empty());

    let j = press(&mut engine, &cfg, &rules, Key::J, &[]);
    assert!(j.swallow);
    assert_eq!(j.fired[0].trigger, "F&J");
}

#[test]
fn incomplete_chord_does_not_fire_and_loses_nothing() {
    let (cfg, rules) = setup(vec![item("chord", &["F&J"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    let f = press(&mut engine, &cfg, &rules, Key::F, &[]);
    assert!(!f.swallow);
    let _ = release(&mut engine, &cfg, &rules, Key::F);
    let a = press(&mut engine, &cfg, &rules, Key::A, &[]);
    assert!(!a.swallow, "没凑齐的和弦不该影响后续按键");
    assert!(a.fired.is_empty());
}

#[test]
fn sequence_passes_leader_and_swallows_the_last_step() {
    let (cfg, rules) = setup(vec![item("seq", &["F9 Space"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    let leader = press(&mut engine, &cfg, &rules, Key::F9, &[]);
    assert!(!leader.swallow, "leader 放行（桌面版靠回放，安卓没有回放）");
    let _ = release(&mut engine, &cfg, &rules, Key::F9);

    let last = press(&mut engine, &cfg, &rules, Key::Space, &[]);
    assert!(last.swallow);
    assert_eq!(last.fired[0].trigger, "F9 Space");
}

#[test]
fn sequence_break_falls_through_to_shortcut() {
    let (cfg, rules) = setup(vec![
        item("seq", &["F9 Space"], vec![url("a")]),
        item("plain", &["A"], vec![url("b")]),
    ]);
    let mut engine = AndroidEngine::new();

    let leader = press(&mut engine, &cfg, &rules, Key::F9, &[]);
    assert!(!leader.swallow);
    // 断链：A 既不是序列下一步，也要能命中自己的快捷键。
    let a = press(&mut engine, &cfg, &rules, Key::A, &[]);
    assert!(a.swallow);
    assert_eq!(a.fired[0].name, "plain");
}

#[test]
fn remap_is_reported_but_never_swallowed() {
    let cfg = Config {
        remaps: vec![kada_core::Remap {
            from: "A".into(),
            to: "B".into(),
            enabled: true,
            ..Default::default()
        }],
        ..Config::default()
    };
    let rules = Rules::build(&cfg);
    let mut engine = AndroidEngine::new();

    let a = press(&mut engine, &cfg, &rules, Key::A, &[]);
    assert!(!a.swallow, "改键要注入，安卓做不到——绝不吞（吞了就丢键）");
    assert!(a.note.contains("remap:unsupported"), "note={}", a.note);
}

#[test]
fn repeat_of_swallowed_key_stays_swallowed() {
    let (cfg, rules) = setup(vec![item("f8", &["F8"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    assert!(press(&mut engine, &cfg, &rules, Key::F8, &[]).swallow);
    let repeat = engine.on_key(&cfg, &rules, Key::F8, true, true, BTreeSet::new(), None);
    assert!(repeat.swallow);
    assert!(repeat.fired.is_empty(), "连发不该重复触发");

    // 没吞过的键连发原样放行。
    let plain = engine.on_key(&cfg, &rules, Key::B, true, true, BTreeSet::new(), None);
    assert!(!plain.swallow);
}

#[test]
fn disabled_and_layered_shortcuts_are_ignored() {
    let mut disabled = item("off", &["F8"], vec![url("a")]);
    disabled.enabled = false;
    let mut layered = item("layer", &["F10"], vec![url("b")]);
    layered.layer = Some("nav".into());
    let (cfg, rules) = setup(vec![disabled, layered]);
    let mut engine = AndroidEngine::new();

    assert!(!press(&mut engine, &cfg, &rules, Key::F8, &[]).swallow);
    assert!(!press(&mut engine, &cfg, &rules, Key::F10, &[]).swallow);
}

#[test]
fn reset_drops_pending_chord_without_firing() {
    let (cfg, rules) = setup(vec![item("chord", &["F&J"], vec![url("a")])]);
    let mut engine = AndroidEngine::new();

    assert!(!press(&mut engine, &cfg, &rules, Key::F, &[]).swallow);
    engine.reset();
    let j = press(&mut engine, &cfg, &rules, Key::J, &[]);
    assert!(!j.swallow, "复位后旧的候选不该还在");
    assert!(j.fired.is_empty());
}
