//! The event -> perceive path: a user message must show up in what the agent sees.

use twin_runtime::JsGraph;

#[test]
fn user_message_is_perceived() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"say","args":{"text":"hi"}}"#);
    g.twin_event(r#"{"type":"user_message","text":"My data is at /tmp/turbines.csv"}"#);
    let seen = g.twin_perceive();
    assert!(
        seen.contains("/tmp/turbines.csv"),
        "perceive() must include the user's message; got: {seen}"
    );
}

#[test]
fn record_profile_is_perceived() {
    let mut g = JsGraph::new_twin();
    g.twin_agent_tool(r#"{"tool":"record_profile","args":{"field":"role","value":"maintenance engineer"}}"#);
    let seen = g.twin_perceive();
    assert!(seen.contains("maintenance engineer"), "got: {seen}");
}
