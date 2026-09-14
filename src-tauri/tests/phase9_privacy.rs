//! Phase 9 acceptance — the remote-content policy (P9.1) and the unsubscribe
//! boundary (P9.4).

use sift::db::messages::MsgUpsert;
use sift::db::Db;
use sift::dto::{MessageRef, RemoteContentMode};
use sift::render::policy;
use sift::unsubscribe::{self, UnsubHeaders, UnsubPlan};

// ---------------------------------------------------------------------------
// P9.1 — remote content policy
// ---------------------------------------------------------------------------

/// A body that tries every way an email can reach the network.
const HOSTILE_BODY: &str = concat!(
    "<img src=\"https://images.example.com/pixel.png\" srcset=\"https://images.example.com/2x.png 2x\">",
    "<img src=\"//cdn.example.com/protocol-relative.png\">",
    "<style>@import url(https://cdn.example.com/sheet.css);",
    "@font-face{font-family:X;src:url(https://fonts.example.com/x.woff2)}",
    ".hero{background-image:url('https://cdn.example.com/hero.png')}</style>",
    "<div style=\"background:url(https://cdn.example.com/inline.png)\">x</div>",
    "<video src=\"https://media.example.com/clip.mp4\" poster=\"https://media.example.com/p.jpg\" autoplay></video>",
    "<audio src=\"https://media.example.com/sound.mp3\"></audio>",
    "<iframe src=\"https://frames.example.com/f\"></iframe>",
    "<table background=\"https://cdn.example.com/table.png\"><tr><td>y</td></tr></table>",
);

/// Every absolute external address left in a rendered body.
fn external_urls(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("http") {
        let after = &rest[i..];
        if !(after.starts_with("http://") || after.starts_with("https://")) {
            rest = &after[4..];
            continue;
        }
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>'))
            .unwrap_or(after.len());
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

#[test]
fn p9_t01_blocked_rendering_has_no_external_target() {
    let sanitized = sift::render::sanitize::sanitize("a1/m1", HOSTILE_BODY);
    // The stored body stays policy-neutral: the policy is applied per read.
    assert!(
        !sanitized.html.contains("@import"),
        "external stylesheets are blocked in every policy"
    );
    assert!(
        !sanitized.html.contains("@font-face"),
        "remote fonts are blocked in every policy"
    );
    assert!(!sanitized.html.contains("<iframe"));

    let blocked = policy::block_remote_content(&sanitized.html);
    let leaks = external_urls(&blocked);
    assert!(
        leaks.is_empty(),
        "blocked read still points outside: {leaks:?}"
    );
    assert!(!blocked.contains("autoplay"));
    assert!(
        blocked.contains(policy::BLOCKED_PIXEL),
        "a blocked image keeps the layout with a local pixel"
    );

    let allowed = policy::allow_remote_content(&sanitized.html);
    let permitted = external_urls(&allowed);
    assert!(
        permitted
            .iter()
            .any(|u| u.contains("images.example.com/pixel.png")),
        "allow must load the images a message needs: {permitted:?}"
    );
    assert!(
        permitted
            .iter()
            .any(|u| u.contains("cdn.example.com/hero.png")),
        "a CSS background follows the same permission as an image tag"
    );
    assert!(!allowed.contains("@import"));
    assert!(!allowed.contains("@font-face"));
    assert!(!allowed.contains("autoplay"));
    assert!(!allowed.contains("fonts.example.com"));
}

#[tokio::test]
async fn p9_t02_a_new_installation_asks_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let settings = db.settings_get().await.unwrap();
    assert_eq!(settings.remote_content_mode, "ask");
    assert!(!settings.remote_content_choice_pending);
    let state = db.privacy_state().await.unwrap();
    assert_eq!(state.stored_mode, RemoteContentMode::Ask);
    assert_eq!(state.effective(), RemoteContentMode::Ask);
    assert!(!state.loads_without_asking());
}

#[tokio::test]
async fn p9_t03_upgrade_preserves_a_deliberate_never_as_block() {
    let dir = tempfile::tempdir().unwrap();
    let db = legacy_database(dir.path(), "never").await;
    let state = db.privacy_state().await.unwrap();
    assert_eq!(state.stored_mode, RemoteContentMode::Block);
    assert!(!state.choice_pending, "a deliberate Never is not ambiguous");
    assert_eq!(state.effective(), RemoteContentMode::Block);
    // The unrelated settings survive the upgrade.
    assert_eq!(db.settings_get().await.unwrap().theme, "dark");
}

#[tokio::test]
async fn p9_t04_upgrade_keeps_ambiguous_behaviour_behind_one_choice() {
    let dir = tempfile::tempdir().unwrap();
    let db = legacy_database(dir.path(), "always").await;
    let state = db.privacy_state().await.unwrap();
    assert!(state.choice_pending, "an ambiguous Always asks once");
    assert_eq!(
        state.effective(),
        RemoteContentMode::Allow,
        "behaviour must not change silently on upgrade"
    );
    assert!(state.loads_without_asking());

    // Answering the one-time choice clears the prompt and stores the decision.
    db.privacy_set_mode(RemoteContentMode::Ask).await.unwrap();
    let state = db.privacy_state().await.unwrap();
    assert!(!state.choice_pending);
    assert_eq!(state.stored_mode, RemoteContentMode::Ask);
    assert_eq!(state.effective(), RemoteContentMode::Ask);
}

#[tokio::test]
async fn p9_t05_permission_generation_moves_with_every_preference_change() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let first = db.new_account("one@example.com", None, None).await.unwrap();
    let second = db.new_account("two@example.com", None, None).await.unwrap();
    let g1 = db.privacy_generation(&first.id).await.unwrap();
    let g2 = db.privacy_generation(&second.id).await.unwrap();
    assert_eq!((g1, g2), (0, 0));

    db.sender_allow(&first.id, "news@example.com")
        .await
        .unwrap();
    db.privacy_bump_generations(Some(std::slice::from_ref(&first.id)))
        .await
        .unwrap();
    assert_eq!(db.privacy_generation(&first.id).await.unwrap(), 1);
    assert_eq!(
        db.privacy_generation(&second.id).await.unwrap(),
        0,
        "one account's sender preference does not invalidate another's cache"
    );

    // A global policy change invalidates every account.
    db.privacy_set_mode(RemoteContentMode::Block).await.unwrap();
    assert_eq!(db.privacy_generation(&first.id).await.unwrap(), 2);
    assert_eq!(db.privacy_generation(&second.id).await.unwrap(), 1);
}

#[tokio::test]
async fn p9_t06_sender_memory_is_account_scoped_and_revocable() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let first = db.new_account("one@example.com", None, None).await.unwrap();
    let second = db.new_account("two@example.com", None, None).await.unwrap();
    db.sender_allow(&first.id, "News@Example.com")
        .await
        .unwrap();

    assert!(db
        .sender_allowed(&first.id, "news@example.com")
        .await
        .unwrap());
    assert!(!db
        .sender_allowed(&first.id, "other@example.com")
        .await
        .unwrap());
    assert!(
        !db.sender_allowed(&second.id, "news@example.com")
            .await
            .unwrap(),
        "a remembered sender never leaks across accounts"
    );
    // It survives a restart...
    drop(db);
    let db = Db::open(dir.path()).unwrap();
    assert!(db
        .sender_allowed(&first.id, "news@example.com")
        .await
        .unwrap());
    assert_eq!(
        db.sender_allow_list(&first.id).await.unwrap(),
        vec!["news@example.com".to_string()]
    );
    // ...and can be revoked.
    db.sender_revoke(&first.id, "news@example.com")
        .await
        .unwrap();
    assert!(!db
        .sender_allowed(&first.id, "news@example.com")
        .await
        .unwrap());
    assert!(db.sender_allow_list(&first.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn p9_t07_load_once_is_a_session_permission_not_a_setting() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    let state = sift::app_state::AppState::new(db.clone(), dir.path().to_path_buf());

    assert!(!state.session_remote_allowed(&account.id, "m1").await);
    state.grant_session_remote_load(&account.id, "m1").await;
    assert!(state.session_remote_allowed(&account.id, "m1").await);
    assert!(
        !state.session_remote_allowed(&account.id, "m2").await,
        "a load-once grant is per message"
    );
    assert!(state.session_privacy_bumps(&account.id).await >= 1);

    // Nothing was written: a fresh runtime over the same database has no grant
    // and the stored policy is untouched.
    let reopened = sift::app_state::AppState::new(db.clone(), dir.path().to_path_buf());
    assert!(!reopened.session_remote_allowed(&account.id, "m1").await);
    assert_eq!(db.settings_get().await.unwrap().remote_content_mode, "ask");
    assert!(db.sender_allow_list(&account.id).await.unwrap().is_empty());
}

/// Seed one message whose body is already cached, so reading it needs no
/// provider and no network.
async fn seed_cached_body(db: &Db, account_id: &str, message_id: &str, html: &str) {
    db.messages_upsert(MsgUpsert {
        id: message_id.to_string(),
        account_id: account_id.to_string(),
        thread_id: "t1".into(),
        internal_date: 10,
        from_name: Some("News".into()),
        from_email: Some("news@example.com".into()),
        subject: "newsletter".into(),
        label_ids: vec!["INBOX".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    let sanitized = sift::render::sanitize::sanitize(&format!("{account_id}/{message_id}"), html);
    db.bodies_put(sift::db::bodies::BodyPut {
        account_id: account_id.to_string(),
        message_id: message_id.to_string(),
        html: Some(sanitized.html),
        text: Some("newsletter".into()),
        remote_images: sanitized.remote_images,
        trackers: sanitized.trackers,
        dark_safe: sanitized.dark_safe,
        quoted_from: None,
        unsubscribe: Default::default(),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p9_t08_reading_a_body_obeys_the_policy_and_reports_broken_inline_images() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    let state = sift::app_state::AppState::new(db.clone(), dir.path().to_path_buf());
    // A message whose inline image has no cached bytes: a broken CID.
    seed_cached_body(
        &db,
        &account.id,
        "m1",
        "<p>hello</p><img src=\"cid:missing-part\"><img src=\"https://images.example.com/a.png\">",
    )
    .await;

    // Default policy is `ask`: nothing external loads until the user decides.
    let body = sift::commands::threads::message_body_for(&state, &account.id, "m1")
        .await
        .unwrap();
    assert_eq!(body.state, "ready");
    assert_eq!(body.remote_content_mode, "ask");
    assert!(!body.remote_images_allowed);
    let html = body.html.clone().unwrap();
    assert!(
        external_urls(&html).is_empty(),
        "an unresolved CID must not be worked around by allowing remote content: {html}"
    );
    assert!(
        body.unresolved_inline_count >= 1,
        "the broken inline reference is reported instead"
    );
    assert_eq!(body.render_version, sift::dto::RENDER_VERSION);

    // "Load once" for this message permits the image on the next read.
    sift::commands::threads::remote_content_allow_for(&state, &account.id, "m1", false)
        .await
        .unwrap();
    let body = sift::commands::threads::message_body_for(&state, &account.id, "m1")
        .await
        .unwrap();
    assert!(body.remote_images_allowed);
    let html = body.html.clone().unwrap();
    assert!(html.contains("https://images.example.com/a.png"), "{html}");
}

#[tokio::test]
async fn p9_t09_a_remembered_sender_changes_the_read_without_a_policy_change() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    let state = sift::app_state::AppState::new(db.clone(), dir.path().to_path_buf());
    seed_cached_body(
        &db,
        &account.id,
        "m1",
        "<img src=\"https://images.example.com/a.png\">",
    )
    .await;
    let before = db.privacy_generation(&account.id).await.unwrap();
    let body = sift::commands::threads::message_body_for(&state, &account.id, "m1")
        .await
        .unwrap();
    assert!(!body.remote_images_allowed);

    db.sender_allow(&account.id, "news@example.com")
        .await
        .unwrap();
    db.privacy_bump_generations(Some(std::slice::from_ref(&account.id)))
        .await
        .unwrap();
    let body = sift::commands::threads::message_body_for(&state, &account.id, "m1")
        .await
        .unwrap();
    assert!(
        body.remote_images_allowed,
        "an allow-listed sender loads without asking"
    );
    assert!(
        body.privacy_generation > before,
        "the permission generation moved with the preference"
    );

    // The same sender in another account is not allowed.
    let other = db.new_account("two@example.com", None, None).await.unwrap();
    seed_cached_body(
        &db,
        &other.id,
        "m1",
        "<img src=\"https://images.example.com/a.png\">",
    )
    .await;
    let body = sift::commands::threads::message_body_for(&state, &other.id, "m1")
        .await
        .unwrap();
    assert!(!body.remote_images_allowed);
}

#[tokio::test]
async fn p9_t10_block_mode_loads_nothing_even_for_a_remembered_sender() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    let state = sift::app_state::AppState::new(db.clone(), dir.path().to_path_buf());
    seed_cached_body(
        &db,
        &account.id,
        "m1",
        "<img src=\"https://images.example.com/a.png\">",
    )
    .await;
    db.sender_allow(&account.id, "news@example.com")
        .await
        .unwrap();
    db.privacy_set_mode(RemoteContentMode::Block).await.unwrap();
    let body = sift::commands::threads::message_body_for(&state, &account.id, "m1")
        .await
        .unwrap();
    assert_eq!(body.remote_content_mode, "block");
    assert!(!body.remote_images_allowed);
    assert!(external_urls(body.html.as_deref().unwrap()).is_empty());

    // Allow loads it without asking.
    db.privacy_set_mode(RemoteContentMode::Allow).await.unwrap();
    let body = sift::commands::threads::message_body_for(&state, &account.id, "m1")
        .await
        .unwrap();
    assert_eq!(body.remote_content_mode, "allow");
    assert!(body.remote_images_allowed);
}

/// A settings document shaped the way a pre-0013 build wrote it: every field
/// it knew about, and neither of the two keys the policy migration adds.
fn legacy_settings(remote_images: &str) -> serde_json::Value {
    let mut value = serde_json::to_value(sift::dto::Settings::default()).unwrap();
    let map = value.as_object_mut().unwrap();
    map.remove("remoteContentMode");
    map.remove("remoteContentChoicePending");
    map.insert("remoteImages".into(), serde_json::json!(remote_images));
    if remote_images == "never" {
        map.insert("theme".into(), serde_json::json!("dark"));
    }
    value
}

/// Build a database whose settings row predates the explicit policy, then run
/// the real 0013 migration SQL over it exactly as the runner would.
async fn legacy_database(dir: &std::path::Path, remote_images: &str) -> Db {
    let db = Db::open(dir).unwrap();
    let json = legacy_settings(remote_images).to_string();
    db.write(move |c| {
        c.execute(
            "INSERT OR REPLACE INTO settings (key,value) VALUES ('settings', ?)",
            rusqlite::params![json],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.write(|c| {
        c.execute_batch(include_str!(
            "../src/db/migrations/0013_remote_content_policy.sql"
        ))?;
        Ok(())
    })
    .await
    .unwrap();
    db
}

// ---------------------------------------------------------------------------
// P9.4 — unsubscribe
// ---------------------------------------------------------------------------

fn targets(raw: &str) -> Vec<sift::dto::UnsubscribeTarget> {
    unsubscribe::parse_targets(raw)
}

#[test]
fn p9_t20_rfc_fixture_plan_table() {
    let dmarc = "mx.google.com; dkim=pass header.d=example.com; dmarc=pass header.from=example.com";
    struct Row {
        name: &'static str,
        header: &'static str,
        post: Option<&'static str>,
        auth: Option<&'static str>,
        trusted: bool,
        from: &'static str,
        expect: Result<&'static str, (&'static str, &'static str)>,
    }
    let rows = [
        Row {
            name: "rfc one-click",
            header: "<mailto:leave@example.com>, <https://example.com/u?id=1>",
            post: Some("List-Unsubscribe=One-Click"),
            auth: Some(dmarc),
            trusted: true,
            from: "news@example.com",
            expect: Ok("https://example.com/u?id=1"),
        },
        Row {
            name: "no post field",
            header: "<https://example.com/u>",
            post: None,
            auth: Some(dmarc),
            trusted: true,
            from: "news@example.com",
            expect: Err(("open_link", "missing_post_value")),
        },
        Row {
            name: "http only",
            header: "<http://example.com/u>",
            post: Some("List-Unsubscribe=One-Click"),
            auth: Some(dmarc),
            trusted: true,
            from: "news@example.com",
            expect: Err(("open_link", "insecure_transport")),
        },
        Row {
            name: "untrusted authentication",
            header: "<https://example.com/u>",
            post: Some("List-Unsubscribe=One-Click"),
            auth: Some(dmarc),
            trusted: false,
            from: "news@example.com",
            expect: Err(("open_link", "untrusted_authentication")),
        },
        Row {
            name: "authentication for another domain",
            header: "<https://example.com/u>",
            post: Some("List-Unsubscribe=One-Click"),
            auth: Some("mx.example.com; dmarc=pass header.from=elsewhere.test"),
            trusted: true,
            from: "news@example.com",
            expect: Err(("open_link", "untrusted_authentication")),
        },
        Row {
            name: "mailto with an encoded subject",
            header: "<mailto:leave@example.com?subject=Unsubscribe%20me>",
            post: None,
            auth: None,
            trusted: false,
            from: "news@example.com",
            expect: Err(("mailto", "")),
        },
        Row {
            name: "malformed header",
            header: "not a url",
            post: Some("List-Unsubscribe=One-Click"),
            auth: Some(dmarc),
            trusted: true,
            from: "news@example.com",
            expect: Err(("none", "")),
        },
        Row {
            name: "empty header",
            header: "",
            post: None,
            auth: None,
            trusted: false,
            from: "news@example.com",
            expect: Err(("none", "")),
        },
    ];
    for row in rows {
        let headers = UnsubHeaders {
            targets: targets(row.header),
            post_value: row.post.map(|p| p.to_string()),
            auth_results: row.auth.map(|a| a.to_string()),
            trusted: row.trusted,
            from_email: row.from.to_string(),
        };
        match (unsubscribe::plan(&headers), row.expect) {
            (UnsubPlan::OneClick { url, .. }, Ok(expected)) => {
                assert_eq!(url, expected, "{}", row.name)
            }
            (UnsubPlan::Fallback(result), Err((method, reason))) => {
                assert_eq!(result.method, method, "{}", row.name);
                assert!(!result.done, "{}", row.name);
                if !reason.is_empty() {
                    assert_eq!(result.reason.as_deref(), Some(reason), "{}", row.name);
                }
            }
            (plan, expected) => panic!("{}: plan {plan:?} did not match {expected:?}", row.name),
        }
    }
}

#[test]
fn p9_t21_mailto_fallback_keeps_its_encoding() {
    let headers = UnsubHeaders {
        targets: targets("<mailto:leave@example.com?subject=Unsubscribe%20me&body=Please%20stop>"),
        post_value: None,
        auth_results: None,
        trusted: false,
        from_email: "news@example.com".into(),
    };
    let UnsubPlan::Fallback(result) = unsubscribe::plan(&headers) else {
        panic!("a mailto-only header is never POSTed to")
    };
    assert_eq!(result.method, "mailto");
    let mailto = result.mailto.unwrap();
    assert!(mailto.contains("subject=Unsubscribe%20me"), "{mailto}");
    assert!(!mailto.contains(" "), "{mailto}");
}

#[tokio::test]
async fn p9_t22_one_click_posts_the_exact_method_header_and_body() {
    use wiremock::matchers::{body_string, header, method, path};
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("POST"))
        .and(path("/u"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("List-Unsubscribe=One-Click"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let url = url::Url::parse(&format!("{}/u?id=1", server.uri())).unwrap();
    let host = url.host_str().unwrap().to_string();
    let address = url.socket_addrs(|| None).unwrap()[0];
    let client = unsubscribe::one_click_client(&host, address).unwrap();
    let outcome = unsubscribe::perform_one_click(&client, &url).await;
    assert!(outcome.delivered, "{outcome:?}");
    assert_eq!(outcome.status, Some(200));
    server.verify().await;
}

#[tokio::test]
async fn p9_t23_a_redirect_off_a_one_click_post_is_not_followed() {
    use wiremock::matchers::{method, path};
    let server = wiremock::MockServer::start().await;
    let target = format!("{}/landing", server.uri());
    wiremock::Mock::given(method("POST"))
        .and(path("/u"))
        .respond_with(
            wiremock::ResponseTemplate::new(302).insert_header("location", target.as_str()),
        )
        .expect(1)
        .mount(&server)
        .await;
    wiremock::Mock::given(method("POST"))
        .and(path("/landing"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let url = url::Url::parse(&format!("{}/u", server.uri())).unwrap();
    let host = url.host_str().unwrap().to_string();
    let address = url.socket_addrs(|| None).unwrap()[0];
    let client = unsubscribe::one_click_client(&host, address).unwrap();
    let outcome = unsubscribe::perform_one_click(&client, &url).await;
    assert!(!outcome.delivered);
    assert_eq!(outcome.reason.as_deref(), Some("redirected"));
    assert_eq!(outcome.status, Some(302));
    server.verify().await;
}

#[tokio::test]
async fn p9_t24_a_failed_post_is_reported_and_falls_back() {
    use wiremock::matchers::{method, path};
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("POST"))
        .and(path("/u"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let url = url::Url::parse(&format!("{}/u", server.uri())).unwrap();
    let host = url.host_str().unwrap().to_string();
    let address = url.socket_addrs(|| None).unwrap()[0];
    let client = unsubscribe::one_click_client(&host, address).unwrap();
    let outcome = unsubscribe::perform_one_click(&client, &url).await;
    assert!(!outcome.delivered);
    assert_eq!(outcome.reason.as_deref(), Some("post_failed"));
    assert_eq!(outcome.status, Some(500));
    server.verify().await;

    // The fallback keeps the link for the user and says what happened.
    let fallback = unsubscribe::fallback_result(
        &targets(&format!("<{}/u>", server.uri())),
        outcome.reason,
        outcome.status,
        Some(outcome.detail),
    );
    assert_eq!(fallback.method, "open_link");
    assert!(!fallback.done);
    drop(server);
}

#[tokio::test]
async fn p9_t25_loopback_private_and_link_local_targets_are_refused() {
    use wiremock::matchers::method;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    // A loopback address (what a hostname resolving to localhost also gives).
    let loopback = url::Url::parse(&format!("{}/u", server.uri())).unwrap();
    let error = unsubscribe::resolve_public(&loopback).await.unwrap_err();
    assert_eq!(error.reason(), "blocked_target");
    // A name that resolves to loopback is refused the same way.
    let named = url::Url::parse("https://localhost/u").unwrap();
    assert_eq!(
        unsubscribe::resolve_public(&named)
            .await
            .unwrap_err()
            .reason(),
        "blocked_target"
    );
    for target in [
        "https://127.0.0.1/u",
        "https://10.0.0.5/u",
        "https://192.168.1.9/u",
        "https://169.254.1.1/u",
        "https://[::1]/u",
        "https://[fe80::1]/u",
        "https://[fc00::1]/u",
        "https://[::ffff:127.0.0.1]/u",
        "https://[::ffff:10.0.0.1]/u",
    ] {
        let parsed = url::Url::parse(target).unwrap();
        let error = unsubscribe::resolve_public(&parsed).await.unwrap_err();
        assert_eq!(error.reason(), "blocked_target", "{target}");
    }
    // Nothing was ever sent.
    assert!(server.received_requests().await.unwrap().is_empty());
    server.verify().await;
}

#[tokio::test]
async fn p9_t26_unsubscribe_from_a_message_never_acts_without_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    db.messages_upsert(MsgUpsert {
        id: "m1".into(),
        account_id: account.id.clone(),
        thread_id: "t1".into(),
        internal_date: 1,
        from_email: Some("news@example.com".into()),
        subject: "newsletter".into(),
        list_unsubscribe: Some(
            "<https://10.0.0.5/u>, <mailto:leave@example.com?subject=Unsubscribe%20me>".into(),
        ),
        list_unsubscribe_post: true,
        list_unsubscribe_post_value: Some("List-Unsubscribe=One-Click".into()),
        label_ids: vec!["INBOX".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    // One-click is advertised but nothing vouches for the headers, so no POST
    // is planned: the user gets the link instead.
    let result = sift::commands::search::unsubscribe_for(&db, &account.id, "m1")
        .await
        .unwrap();
    assert_eq!(result.method, "open_link");
    assert!(!result.done);
    assert_eq!(result.reason.as_deref(), Some("untrusted_authentication"));
    assert_eq!(result.url.as_deref(), Some("https://10.0.0.5/u"));
    assert_eq!(result.targets.len(), 2);
    assert_eq!(result.targets[0].scheme, "https");

    // With provider evidence the POST is planned, and the private address is
    // refused before anything is sent (10.0.0.5 resolves without DNS).
    db.write({
        let account = account.id.clone();
        move |c| {
            c.execute(
                "UPDATE messages SET auth_results='mx.google.com; dmarc=pass header.from=example.com', auth_results_trusted=1 WHERE account_id=? AND id='m1'",
                rusqlite::params![account],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    let plan = unsubscribe::plan(&UnsubHeaders {
        targets: targets("<https://10.0.0.5/u>"),
        post_value: Some("List-Unsubscribe=One-Click".into()),
        auth_results: Some("mx.google.com; dmarc=pass header.from=example.com".into()),
        trusted: true,
        from_email: "news@example.com".into(),
    });
    assert!(
        matches!(plan, UnsubPlan::OneClick { .. }),
        "provider evidence for the From domain authorises the attempt"
    );
    let result = sift::commands::search::unsubscribe_for(&db, &account.id, "m1")
        .await
        .unwrap();
    assert!(!result.done);
    assert_eq!(result.reason.as_deref(), Some("blocked_target"));
    assert_eq!(result.method, "open_link");
}

#[tokio::test]
async fn p9_t27_a_mailto_only_header_produces_a_compose_flow() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    db.messages_upsert(MsgUpsert {
        id: "m1".into(),
        account_id: account.id.clone(),
        thread_id: "t1".into(),
        internal_date: 1,
        subject: "newsletter".into(),
        list_unsubscribe: Some("<mailto:leave@example.com?subject=Unsubscribe%20me>".into()),
        label_ids: vec!["INBOX".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    let result = sift::commands::search::unsubscribe_for(&db, &account.id, "m1")
        .await
        .unwrap();
    assert_eq!(result.method, "mailto");
    assert!(!result.done);
    let mailto = result.mailto.clone().unwrap();
    assert!(mailto.starts_with("mailto:leave@example.com"), "{mailto}");
    assert!(mailto.contains("subject=Unsubscribe%20me"), "{mailto}");
}

#[tokio::test]
async fn p9_t28_a_message_without_an_unsubscribe_header_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("one@example.com", None, None).await.unwrap();
    db.messages_upsert(MsgUpsert {
        id: "m1".into(),
        account_id: account.id.clone(),
        thread_id: "t1".into(),
        internal_date: 1,
        subject: "no header".into(),
        label_ids: vec!["INBOX".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    let result = sift::commands::search::unsubscribe_for(&db, &account.id, "m1")
        .await
        .unwrap();
    assert_eq!(result.method, "none");
    assert!(!result.done);
    assert_eq!(result.reason.as_deref(), Some("no_targets"));
    assert!(result.targets.is_empty());

    // A message that does not exist in this account is not found; the provider
    // id alone is not a cross-account key.
    let other = db.new_account("two@example.com", None, None).await.unwrap();
    let missing = MessageRef::new(other.id.clone(), "m1");
    assert!(
        sift::commands::search::unsubscribe_for(&db, &missing.account_id, "m1")
            .await
            .is_err()
    );
}
