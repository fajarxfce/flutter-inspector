//! Read-only discovery diagnostics; URLs and application data are not printed.
use inspector_adapters::discovery::Discovery;
use inspector_application::BridgeStatus;

#[tokio::main]
async fn main() {
    let mut discovery = Discovery::default();
    let report = discovery.scan().await;
    for app in report.apps {
        let status = match app.bridge {
            BridgeStatus::Ready => {
                if app.busy {
                    "in use".into()
                } else {
                    "ready".into()
                }
            }
            BridgeStatus::Native => "ready (built-in debug APIs)".into(),
            BridgeStatus::Missing => "debug APIs unavailable".into(),
            BridgeStatus::Unavailable(error) => format!("VM unavailable: {error}"),
        };
        println!("{} | {} | {status}", app.name, app.device);
    }
    for note in report.notes {
        println!("{note}");
    }
    discovery.shutdown().await;
}
