use super::*;
use fxg_protocol::common::{AgentAuthMethodKind, ElicitationValue};
use std::collections::BTreeMap;

#[test]
fn build_elicitation_content_parses_json_and_set_pairs() {
    let content = build_elicitation_content(
        &Some("{\"strategy\":\"a\"}".to_owned()),
        &["count=3".to_owned(), "note=hello".to_owned()],
    )
    .expect("content");
    assert_eq!(
        content.get("strategy"),
        Some(&ElicitationValue::String("a".to_owned()))
    );
    assert_eq!(content.get("count"), Some(&ElicitationValue::Integer(3)));
    // Values that do not parse as JSON are treated as strings
    assert_eq!(
        content.get("note"),
        Some(&ElicitationValue::String("hello".to_owned()))
    );
}

#[test]
fn build_elicitation_content_rejects_non_object_json() {
    let err = build_elicitation_content(&Some("[1,2]".to_owned()), &[]).expect_err("array");
    assert!(err.to_string().contains("JSON object"), "got: {err}");
}

#[test]
fn build_elicitation_content_requires_key_value_pairs() {
    let err = build_elicitation_content(&None, &["invalid".to_owned()]).expect_err("pair");
    assert!(err.to_string().contains("key=value"), "got: {err}");
}

fn auth_method(id: &str, kind: AgentAuthMethodKind) -> fxg_protocol::common::AgentAuthMethodInfo {
    fxg_protocol::common::AgentAuthMethodInfo {
        id: id.to_owned(),
        name: id.to_owned(),
        description: None,
        kind,
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}

#[test]
fn choose_auth_method_prefers_terminal() {
    let methods = vec![
        auth_method("agent-login", AgentAuthMethodKind::Agent),
        auth_method("terminal-login", AgentAuthMethodKind::Terminal),
    ];
    let chosen = choose_auth_method(&methods, None).expect("terminal method");
    assert_eq!(chosen.id, "terminal-login");
}

#[test]
fn choose_auth_method_honors_explicit_id() {
    let methods = vec![
        auth_method("agent-login", AgentAuthMethodKind::Agent),
        auth_method("terminal-login", AgentAuthMethodKind::Terminal),
    ];
    let chosen = choose_auth_method(&methods, Some("agent-login")).expect("explicit method");
    assert_eq!(chosen.id, "agent-login");
}

#[test]
fn choose_auth_method_requires_a_choice_when_ambiguous() {
    let methods = vec![
        auth_method("a", AgentAuthMethodKind::Agent),
        auth_method("b", AgentAuthMethodKind::Agent),
    ];
    let err = choose_auth_method(&methods, None).expect_err("ambiguous");
    assert!(err.to_string().contains("--method"), "got: {err}");
}
