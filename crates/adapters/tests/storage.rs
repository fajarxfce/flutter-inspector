use inspector_adapters::{
    har,
    store::{Store, now},
};
use inspector_application::{HistoryTarget, Settings};
use inspector_domain::*;

fn exchange(id: &str) -> Exchange {
    Exchange {
        id: id.into(),
        started_at: now(),
        client_id: "api".into(),
        request: Request {
            method: "POST".into(),
            url: "https://example.test/a?q=1".into(),
            headers: vec![
                Header {
                    name: "Authorization".into(),
                    value: "Bearer original".into(),
                },
                Header {
                    name: "X-Repeat".into(),
                    value: "one".into(),
                },
                Header {
                    name: "X-Repeat".into(),
                    value: "two".into(),
                },
            ],
            body: Some(Body {
                id: format!("{id}-req"),
                size: 4,
                complete: true,
                content_type: "application/octet-stream".into(),
            }),
        },
        response: Some(Response {
            status: 201,
            headers: vec![],
            body: Some(Body {
                id: format!("{id}-res"),
                size: 13,
                complete: false,
                content_type: "application/json".into(),
            }),
        }),
        state: ExchangeState::Complete,
        ..Default::default()
    }
}
#[test]
fn har_round_trip_preserves_headers_binary_large_json_number_and_truncation() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let store = Store::open(source.path().into()).unwrap();
    let mut e = exchange("a");
    let response = br#"{"id":9007199254740993}"#;
    e.response.as_mut().unwrap().body.as_mut().unwrap().size = response.len() as u64;
    store.put(&e).unwrap();
    store.put_body("a-req", &[0, 255, 128, 1]).unwrap();
    store.put_body("a-res", response).unwrap();
    let path = source.path().join("test.har");
    har::export(&store, &path).unwrap();
    let other = Store::open(destination.path().into()).unwrap();
    assert_eq!(har::import(&other, &path).unwrap(), 1);
    let imported = other.history("", 0).unwrap().0.remove(0);
    assert_eq!(imported.request.headers, e.request.headers);
    assert_eq!(
        other.body(&imported.request.body.unwrap().id).unwrap(),
        [0, 255, 128, 1]
    );
    let body = imported.response.unwrap().body.unwrap();
    assert!(!body.complete);
    assert_eq!(other.body(&body.id).unwrap(), response);
}
#[test]
fn retention_removes_raw_files_but_preserves_pins_and_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    for (id, pinned) in [("old", false), ("saved", true)] {
        let mut e = exchange(id);
        e.started_at = 1;
        e.pinned = pinned;
        store.put(&e).unwrap();
        store.put_body(&format!("{id}-req"), &[0; 4]).unwrap();
    }
    let draft = Draft {
        body: "secret draft".into(),
        ..Default::default()
    };
    store.save_draft(&draft).unwrap();
    assert!(store.prune(&Settings::default()).unwrap());
    assert!(!store.body_exists("old-req"));
    assert!(store.body_exists("saved-req"));
    assert_eq!(store.history("", 0).unwrap().1, 1);
    assert_eq!(store.drafts().unwrap()[0].body, draft.body);
    store
        .put_body("saved-res", &vec![0; 2 * 1024 * 1024])
        .unwrap();
    assert!(
        !store
            .prune(&Settings {
                quota_bytes: 1024 * 1024,
                ..Default::default()
            })
            .unwrap()
    );
    assert_eq!(store.history("", 0).unwrap().1, 1);
}
#[test]
fn paged_search_handles_filters_and_literal_wildcards() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    for n in 0..225 {
        store.put(&exchange(&n.to_string())).unwrap();
    }
    assert_eq!(store.history("m:post s:20 api", 0).unwrap().0.len(), 200);
    assert_eq!(store.history("m:post s:20 api", 200).unwrap().0.len(), 25);
    assert_eq!(store.history("%", 0).unwrap().1, 0);
    assert_eq!(store.history("m:GET", 0).unwrap().1, 0);
}
#[test]
fn future_schema_is_rejected_without_downgrading() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(dir.path().join("inspector.sqlite")).unwrap();
    db.execute_batch("PRAGMA user_version=99;").unwrap();
    drop(db);
    assert!(Store::open(dir.path().into()).is_err());
    let db = rusqlite::Connection::open(dir.path().join("inspector.sqlite")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        99
    );
}

#[test]
fn har_missing_body_is_explicitly_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    let mut e = exchange("missing");
    e.state = ExchangeState::Error;
    e.error = Some("capture disconnected".into());
    store.put(&e).unwrap();
    let path = dir.path().join("missing.har");
    har::export(&store, &path).unwrap();
    let archive: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let metadata = &archive["log"]["entries"][0]["_inspector"];
    assert_eq!(metadata["requestComplete"], false);
    assert_eq!(metadata["responseComplete"], false);
    assert_eq!(metadata["state"], "error");
}

#[test]
fn clear_item_preserves_shared_bodies_and_drafts_and_rejects_late_updates() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    let first = exchange("one");
    let mut other = exchange("two");
    other.request.body = first.request.body.clone();
    store.put(&first).unwrap();
    store.put(&other).unwrap();
    store.put_body("one-req", b"shared").unwrap();
    store.put_body("one-res", b"removed").unwrap();
    store.pin("two", true).unwrap();
    store
        .save_draft(&Draft {
            id: "saved".into(),
            body: "keep this".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store.clear_history(&HistoryTarget::single("one")).unwrap(),
        1
    );
    assert!(
        !store.put(&first).unwrap(),
        "A late update cannot restore the cleared item"
    );
    assert!(store.body_exists("one-req"), "Another row shares this body");
    assert!(!store.body_exists("one-res"));
    assert!(!store.put_body("one-res", b"late download").unwrap());
    assert!(!store.body_exists("one-res"));
    let remaining = store.history("", 0).unwrap().0;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "two");
    assert!(remaining[0].pinned);
    assert_eq!(store.drafts().unwrap()[0].body, "keep this");
    assert_eq!(
        store
            .clear_history(&HistoryTarget::single("missing"))
            .unwrap(),
        0
    );
}

#[test]
fn clear_all_survives_reopen_and_new_capture_continues() {
    let dir = tempfile::tempdir().unwrap();
    let first = exchange("old");
    {
        let store = Store::open(dir.path().into()).unwrap();
        store.put(&first).unwrap();
        store.pin("old", true).unwrap();
        store.put_body("old-req", b"bytes").unwrap();
        store
            .save_draft(&Draft {
                id: "saved".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            store.clear_history(&HistoryTarget::All).unwrap(),
            1,
            "Clear all includes pinned history"
        );
        assert_eq!(store.history("", 0).unwrap().1, 0);
        assert_eq!(
            std::fs::read_dir(dir.path().join("bodies"))
                .unwrap()
                .count(),
            0
        );
    }
    let store = Store::open(dir.path().into()).unwrap();
    assert!(
        !store.put(&first).unwrap(),
        "Reconnect cannot restore old capture"
    );
    assert!(!store.put_body("old-req", b"late").unwrap());
    assert!(store.put(&exchange("new")).unwrap());
    assert!(store.put_body("new-req", b"fresh").unwrap());
    assert_eq!(store.history("", 0).unwrap().1, 1);
    assert_eq!(store.drafts().unwrap().len(), 1);
}

#[test]
fn version_one_history_migrates_without_losing_records() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path().into()).unwrap();
        store.put(&exchange("keep")).unwrap();
    }
    let db = rusqlite::Connection::open(dir.path().join("inspector.sqlite")).unwrap();
    db.execute_batch("DROP TABLE cleared_exchanges; PRAGMA user_version=1;")
        .unwrap();
    drop(db);
    let store = Store::open(dir.path().into()).unwrap();
    assert_eq!(store.history("", 0).unwrap().1, 1);
    assert_eq!(
        store.clear_history(&HistoryTarget::single("keep")).unwrap(),
        1
    );
}

#[test]
fn clear_selected_only_removes_requested_ids_and_their_unshared_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    let first = exchange("one");
    let second = exchange("two' OR 1=1 --");
    let mut keep = exchange("keep");
    keep.request.body = first.request.body.clone();
    for item in [&first, &second, &keep] {
        store.put(item).unwrap();
    }
    store.pin(&second.id, true).unwrap();
    let second_body = &second.request.body.as_ref().unwrap().id;
    store.put_body("one-req", b"shared").unwrap();
    store.put_body("one-res", b"removed").unwrap();
    store.put_body(second_body, b"removed too").unwrap();
    store
        .save_draft(&Draft {
            id: "saved".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store
            .clear_history(&HistoryTarget::Items(Default::default()))
            .unwrap(),
        0
    );
    assert_eq!(store.history("", 0).unwrap().1, 3);
    let target = HistoryTarget::Items(
        [
            first.id.clone(),
            second.id.clone(),
            first.id.clone(),
            "missing".into(),
        ]
        .into(),
    );
    assert_eq!(store.clear_history(&target).unwrap(), 2);
    let remaining = store.history("", 0).unwrap().0;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, keep.id);
    assert!(store.body_exists("one-req"));
    assert!(!store.body_exists("one-res"));
    assert!(!store.body_exists(second_body));
    assert_eq!(store.drafts().unwrap().len(), 1);
    drop(store);
    let store = Store::open(dir.path().into()).unwrap();
    assert!(!store.put(&first).unwrap());
    assert!(!store.put(&second).unwrap());
    assert!(store.put(&keep).unwrap());
}

#[test]
fn selected_clear_rolls_back_every_item_and_marker_if_one_delete_fails() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().into()).unwrap();
    let first = exchange("first");
    let second = exchange("second");
    for item in [&first, &second, &exchange("untouched")] {
        store.put(item).unwrap();
    }
    let db = rusqlite::Connection::open(dir.path().join("inspector.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_selected_delete BEFORE DELETE ON exchanges
        WHEN OLD.id='second' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
    )
    .unwrap();
    let target = HistoryTarget::Items([first.id.clone(), second.id.clone()].into());
    assert!(store.clear_history(&target).is_err());
    assert_eq!(store.history("", 0).unwrap().1, 3);
    assert!(
        store.put(&first).unwrap(),
        "Deletion markers must roll back too"
    );
    assert!(store.put(&second).unwrap());
}
