use inspector_adapters::store::{Store, now};
use inspector_domain::{Exchange, Request, Response};
use std::time::Instant;

fn main() {
    let dir = tempfile::tempdir().expect("temporary store");
    let store = Store::open(dir.path().into()).expect("store");
    let count = std::env::args()
        .nth(1)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50_000);
    for n in 0..count {
        store
            .put(&Exchange {
                id: n.to_string(),
                started_at: now(),
                client_id: "api".into(),
                request: Request {
                    method: if n % 2 == 0 { "POST" } else { "GET" }.into(),
                    url: format!("https://fixture.test/api/items/{n}"),
                    ..Default::default()
                },
                response: Some(Response {
                    status: 200,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .expect("seed");
    }
    let mut timings = vec![];
    for _ in 0..30 {
        let start = Instant::now();
        let result = store.history("m:post s:2 items/42", 0).expect("query");
        assert!(result.0.len() <= 200);
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"rows":count,"samples":timings.len(),"search_p50_ms":timings[15],"search_p95_ms":timings[28],"query":"m:post s:2 items/42"})
    );
}
