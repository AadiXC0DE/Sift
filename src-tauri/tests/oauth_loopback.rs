// P2-T02: loopback state validation + timeout
use std::collections::HashMap;

#[tokio::test]
async fn p2_t02_loopback_state_and_timeout() {
    // Start a raw listener mimicking oauth::run_loopback behavior: use the real start_flow
    let flow = sift::provider::gmail::oauth::start_flow().unwrap();
    let port = flow.port;
    let state = flow.state.clone();
    // wrong state -> 400 and flow keeps waiting
    let wrong = reqwest::get(format!("http://127.0.0.1:{port}/?code=abc&state=wrong"))
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 400);
    // right state -> 200
    let good = reqwest::get(format!("http://127.0.0.1:{port}/?code=abc&state={state}"))
        .await
        .unwrap();
    assert_eq!(good.status().as_u16(), 200);
    let code = flow.wait(5).await.unwrap();
    assert_eq!(code, "abc");

    // timeout
    let flow2 = sift::provider::gmail::oauth::start_flow().unwrap();
    let e = flow2.wait(1).await.unwrap_err();
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["code"], "oauth_timeout");
    let _ = HashMap::<String, String>::new();
}
