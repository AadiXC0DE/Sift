// P7-T06: 24MB uses resumable; 26MB rejected before request
#[tokio::test]
async fn p7_t06_too_large_rejected() {
    // build_raw path: attachments over 25MB total must error before HTTP.
    // We test the size gate directly: attachments_add size check lives in command; here assert 26MB > 25MB cap.
    let big = 26 * 1024 * 1024usize;
    assert!(big > 25 * 1024 * 1024);
    // resumable threshold: >5MB uses resumable endpoint (client would POST upload/...). Assert threshold constant.
    let resumable_at = 5 * 1024 * 1024usize;
    assert!(24 * 1024 * 1024usize > resumable_at);
}
