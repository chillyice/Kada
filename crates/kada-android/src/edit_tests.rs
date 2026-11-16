//! 编辑器后端的单测（宿主机跑）：触发键校验 / 撞车判定都只经 core 解析，Kotlin 不参与。

use kada_core::{Action, Config, ShortcutItem};

use crate::check_trigger;

fn item(name: &str, triggers: &[&str]) -> ShortcutItem {
    ShortcutItem {
        name: Some(name.into()),
        triggers: triggers.iter().map(|t| t.to_string()).collect(),
        actions: vec![Action::Semantic {
            operation: kada_core::SemanticOperation::Copy,
            description: None,
        }],
        enabled: true,
        ..ShortcutItem::default()
    }
}

fn cfg(items: Vec<ShortcutItem>) -> Config {
    Config {
        shortcuts: items,
        ..Config::default()
    }
}

#[test]
fn valid_trigger_reports_kind_and_canonical_form() {
    let cfg = cfg(vec![item("a", &["F8"])]);

    let combo = check_trigger("Ctrl+Alt+K", -1, &cfg);
    assert_eq!(combo["ok"], true);
    assert_eq!(combo["kind"], "combo");
    assert_eq!(combo["canonical"], "Ctrl+Alt+K");
    assert_eq!(combo["normalized"], false);
    assert_eq!(combo["duplicates"].as_array().unwrap().len(), 0);

    assert_eq!(check_trigger("F9 Space", -1, &cfg)["kind"], "sequence");
    assert_eq!(check_trigger("F&J", -1, &cfg)["kind"], "chord");
}

#[test]
fn lowercase_input_is_normalized_but_still_exact() {
    let cfg = cfg(vec![]);

    let loose = check_trigger("ctrl+alt+k", -1, &cfg);
    assert_eq!(loose["ok"], true);
    assert_eq!(loose["canonical"], "Ctrl+Alt+K");
    assert_eq!(
        loose["normalized"], true,
        "补正过大小写要标出来，界面才知道要回填"
    );
}

#[test]
fn invalid_trigger_reports_the_core_error() {
    let cfg = cfg(vec![]);

    let empty = check_trigger("   ", -1, &cfg);
    assert_eq!(empty["ok"], false);
    assert!(empty["error"].as_str().unwrap().contains("不能为空"));

    let unknown = check_trigger("Foo+K", -1, &cfg);
    assert_eq!(unknown["ok"], false);
    assert!(
        unknown["error"].as_str().unwrap().contains("Foo"),
        "{unknown}"
    );
}

#[test]
fn duplicate_detection_ignores_case_and_can_skip_self() {
    let cfg = cfg(vec![item("已有", &["Ctrl+K"]), item("另一条", &["F8"])]);

    let dup = check_trigger("ctrl+k", -1, &cfg);
    let list = dup["duplicates"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["item"], 0);
    assert_eq!(list[0]["name"], "已有");

    // 编辑第 0 条时，它自己的触发键不算撞车。
    let self_edit = check_trigger("Ctrl+K", 0, &cfg);
    assert_eq!(self_edit["duplicates"].as_array().unwrap().len(), 0);

    // 换成别的键自然也不撞。
    assert_eq!(
        check_trigger("Ctrl+J", -1, &cfg)["duplicates"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn duplicate_detection_compares_normalized_form() {
    let cfg = cfg(vec![item("小写写的", &["ctrl+f&j"])]);

    let dup = check_trigger("Ctrl+F&J", -1, &cfg);
    assert_eq!(
        dup["duplicates"].as_array().unwrap().len(),
        1,
        "写法不同但是同一个触发键，必须算撞车"
    );
}
