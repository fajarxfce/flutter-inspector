use inspector_adapters::store::Store;
use inspector_application::HistoryTarget;
use inspector_domain::*;

fn exchange(tab: &str, run: &str) -> Exchange {
    Exchange {
        id: run.into(),
        repeater_id: Some(tab.into()),
        started_at: 1,
        request: Request {
            body: Some(Body {
                id: format!("{run}-request"),
                ..Default::default()
            }),
            ..Default::default()
        },
        response: Some(Response {
            status: 200,
            body: Some(Body {
                id: format!("{run}-response"),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}
#[test]
fn results_survive_clear_restart_and_are_isolated_by_tab_and_send() {
    let temp = tempfile::tempdir().unwrap();
    let one = Draft {
        id: "one".into(),
        ..Default::default()
    };
    let two = Draft {
        id: "two".into(),
        ..Default::default()
    };
    {
        let store = Store::open(temp.path().into()).unwrap();
        let original = Exchange {
            id: "network-original".into(),
            request: Request {
                url: "https://original.test".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        store.put(&original).unwrap();
        for (draft, run) in [(&one, "one-a"), (&two, "two-a")] {
            let exchange = exchange(&draft.id, run);
            store.begin_repeater(draft, &exchange).unwrap();
            store
                .put_body(&format!("{run}-response"), run.as_bytes())
                .unwrap();
        }
        assert_eq!(store.history("", 0).unwrap().1, 1);
        assert_eq!(
            store.history("", 0).unwrap().0[0].request.url,
            "https://original.test"
        );
        store.clear_history(&HistoryTarget::All).unwrap();
        assert_eq!(store.body("one-a-response").unwrap(), b"one-a");
        assert_eq!(store.body("two-a-response").unwrap(), b"two-a");
        let new = exchange("one", "one-b");
        store.begin_repeater(&one, &new).unwrap();
        assert!(!store.put(&exchange("one", "one-a")).unwrap());
        assert!(!store.put_body("one-a-response", b"late").unwrap());
        store.put_body("one-b-response", b"new").unwrap();
        assert!(!store.body_exists("one-a-response"));
    }
    let store = Store::open(temp.path().into()).unwrap();
    assert_eq!(store.repeater_result("one").unwrap().unwrap().id, "one-b");
    assert_eq!(store.body("one-b-response").unwrap(), b"new");
    assert_eq!(store.body("two-a-response").unwrap(), b"two-a");
    store.delete_draft("one").unwrap();
    assert!(!store.put(&exchange("one", "one-b")).unwrap());
    assert!(!store.body_exists("one-b-response"));
    assert!(store.body_exists("two-a-response"));
}

#[test]
fn late_bridge_event_cannot_replace_cancelled_response() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().into()).unwrap();
    let draft = Draft::default();
    let mut result = exchange(&draft.id, "run");
    store.begin_repeater(&draft, &result).unwrap();
    result.state = ExchangeState::Complete;
    assert!(store.put(&result).unwrap());
    result.state = ExchangeState::Pending;
    assert!(!store.put(&result).unwrap());
    assert_eq!(
        store.repeater_result(&draft.id).unwrap().unwrap().state,
        ExchangeState::Complete
    );
    result.state = ExchangeState::Cancelled;
    assert!(store.put(&result).unwrap());
    result.state = ExchangeState::Complete;
    assert!(!store.put(&result).unwrap());
    assert_eq!(
        store.repeater_result(&draft.id).unwrap().unwrap().state,
        ExchangeState::Cancelled
    );
}
