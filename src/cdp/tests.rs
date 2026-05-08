use crate::cdp::transport::CdpTransport;
use crate::cdp::types::JsonRpcMessage;
use serde_json::json;

#[test]
fn test_json_rpc_parsing() {
    let response_json = r#"{"id": 1, "result": {"foo": "bar"}}"#;
    let parsed: JsonRpcMessage = serde_json::from_str(response_json).unwrap();
    match parsed {
        JsonRpcMessage::Response { id, result, .. } => {
            assert_eq!(id, 1);
            assert_eq!(result.unwrap(), json!({"foo": "bar"}));
        }
        _ => panic!("Expected Response"),
    }

    let notification_json = r#"{"method": "Target.targetCreated", "params": {"targetInfo": {}}}"#;
    let parsed: JsonRpcMessage = serde_json::from_str(notification_json).unwrap();
    match parsed {
        JsonRpcMessage::Notification { method, params } => {
            assert_eq!(method, "Target.targetCreated");
            assert!(params.is_some());
        }
        _ => panic!("Expected Notification"),
    }
}

#[test]
fn test_transport_connect_invalid_url() {
    smol::block_on(async {
        let result = CdpTransport::connect("ws://127.0.0.1:1").await;
        assert!(result.is_err());
    });
}

#[test]
fn test_is_cdp_compatible() {
    use crate::cdp::browser::is_cdp_compatible;
    assert!(is_cdp_compatible("google-chrome"));
    assert!(is_cdp_compatible("Chromium"));
    assert!(is_cdp_compatible("Brave-Browser"));
    assert!(is_cdp_compatible("microsoft-edge"));
    assert!(is_cdp_compatible("msedge.exe"));
    assert!(!is_cdp_compatible("firefox"));
    assert!(!is_cdp_compatible("safari"));
}

#[test]
fn test_find_browser_executable_not_empty() {
    use crate::cdp::browser::find_browser_executable;
    let bin = find_browser_executable();
    assert!(!bin.is_empty(), "Should at least return a fallback string");
}
