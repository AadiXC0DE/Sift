//! TRANSPORT GATE (spike, temporary): proves imap-proto 0.16.7 decodes every
//! response shape Sift needs before we commit to async-imap as the session
//! transport. Folded into provider/imap/proto.rs snapshot tests once green.
use imap_proto::{parser::parse_response, types::Response};

fn transcript_debug(name: &str) -> Vec<String> {
    let path = format!("{}/../fixtures/imap/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).unwrap();
    let mut rest: &[u8] = &bytes;
    let mut out = vec![];
    while !rest.iter().all(|b| *b == b'\r' || *b == b'\n') {
        match parse_response(rest) {
            Ok((r, resp)) => {
                out.push(match &resp {
                    Response::Fetch(_, attrs) => format!("FETCH {attrs:?}"),
                    Response::MailboxData(d) => format!("MBDATA {d:?}"),
                    Response::Done { code, .. } => format!("DONE {code:?}"),
                    other => format!("OTHER {other:?}"),
                });
                rest = r;
            }
            Err(e) => panic!(
                "parse failed in {name} at offset {}: {e:?}",
                bytes.len() - rest.len()
            ),
        }
    }
    out
}

#[test]
fn spike_fetch_meta_shapes() {
    let rs = transcript_debug("fetch-meta.txt");
    let fetches: Vec<_> = rs.iter().filter(|r| r.starts_with("FETCH")).collect();
    assert_eq!(fetches.len(), 2, "want 2 FETCH responses: {rs:?}");
    let a1 = fetches[0];
    for want in [
        "Uid(4521)",
        "Seen",
        "ModSeq(89123)",
        "GmailMsgId(1912345678901234567)",
        "GmailThrId(1912345678901234567)",
        "Weekly Reports",
        "Clients/Acme",
    ] {
        assert!(a1.contains(want), "msg1 missing {want}: {a1}");
    }
    let a2 = fetches[1];
    for want in [
        "Uid(4522)",
        "Flagged",
        "GmailThrId(1912345678901234001)",
        "B&APw-ro",
    ] {
        assert!(a2.contains(want), "msg2 missing {want}: {a2}");
    }
}

#[test]
fn spike_fetch_partial_literal() {
    let rs = transcript_debug("fetch-partial.txt");
    assert!(
        rs.iter()
            .any(|r| r.starts_with("FETCH") && r.contains("Uid(4521)")),
        "partial BODY literal must decode: {rs:?}"
    );
    let raw = std::fs::read(format!(
        "{}/../fixtures/imap/fetch-partial.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(raw.windows(6).any(|w| w == b"{2048}"));
}

#[test]
fn spike_list_special_use() {
    let rs = transcript_debug("list-special.txt");
    let lists: Vec<_> = rs.iter().filter(|r| r.starts_with("MBDATA")).collect();
    assert_eq!(lists.len(), 9, "want 9 LIST entries: {rs:?}");
    let all = lists
        .iter()
        .find(|r| r.contains("All") && !r.contains("Noselect"))
        .expect("an \\All entry");
    assert!(
        all.contains("[Gmail]/Alle Nachrichten"),
        "role by attribute, never by name: {all}"
    );
}

#[test]
fn spike_select_status_search() {
    let rs = transcript_debug("select-status.txt");
    let has = |s: &str| rs.iter().any(|r| r.contains(s));
    assert!(has("UidValidity(987654)"), "{rs:?}");
    assert!(has("UidNext(4523)"), "{rs:?}");
    assert!(has("HighestModSeq(89123)"), "{rs:?}");
    assert!(has("Exists(412)"), "{rs:?}");
    assert!(has("Search([1, 2, 3, 4521, 4522])"), "{rs:?}");
    assert!(has("AppendUid"), "{rs:?}");
    assert!(has("ModSeq(89140)"), "{rs:?}");
}
