use openai_api_rs::v1::api::Client;
use serde_json::{json, Value};

/// Keep the original live tests available to CI while also exercising the SDK
/// transport, serialization, and response handling in uncredentialed runs.
pub fn client(name: &str, replies: &[Value]) -> (Client, Option<mockito::ServerGuard>) {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.trim().is_empty() {
            return (Client::new(key), None);
        }
    }
    let mut server = mockito::Server::new();
    for arguments in replies {
        server.mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "tools": [{"type": "function", "function": {"name": name, "parameters": {"type": "object"}}}]
            })))
            .with_body(json!({
                "object":"chat.completion","model":"mock","id":"id","created":0,
                "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2},
                "choices":[{"index":0, "finish_reason":"tool_calls","message":{
                    "role":"assistant","content":null,"tool_calls":[{"type":"function","id":"call",
                    "function":{"name":name,"arguments":arguments.to_string()}}]
                }}]
            }).to_string()).create();
    }
    (
        Client::new_with_endpoint(server.url(), "test".into()),
        Some(server),
    )
}

// The 4.x SDK's minreq transport ignores NO_PROXY. In proxied environments
// run just this loopback test in a child, keeping the parent environment and
// live provider requests intact. Mutating env from test threads is unsafe.
pub fn run_loopback_in_child(test: &str) -> bool {
    let proxies = ["http_proxy", "all_proxy", "ALL_PROXY"];
    if std::env::var("OPENAI_API_KEY").is_ok_and(|s| !s.trim().is_empty())
        || !proxies.iter().any(|k| std::env::var_os(k).is_some())
    {
        return false;
    }
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    for key in proxies {
        cmd.env_remove(key);
    }
    let output = cmd.args(["--exact", test]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
