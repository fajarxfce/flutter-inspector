//! Read-only assertions for GUI integration. Refuses non-fixture applications.
use inspector_adapters::{native::NativeSession, vm::VmClient};
use serde_json::{Value, json};
use std::io::Read;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let input: Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
    let vm = VmClient::connect(input["uri"].as_str().ok_or("Missing URI")?).await?;
    let info = vm.call("getVM", json!({})).await?;
    let isolate = info["isolates"]
        .as_array()
        .ok_or("No isolates")?
        .iter()
        .find(|i| i["name"] == "main")
        .ok_or("No main isolate")?["id"]
        .as_str()
        .ok_or("No isolate id")?;
    let details = vm.call("getIsolate", json!({"isolateId":isolate})).await?;
    if details["rootLib"]["uri"] != "package:inspector_fixture/main.dart" {
        return Err("This helper only accepts the Inspector fixture".into());
    }
    let (native, hello) =
        NativeSession::open_with_network(vm.clone(), isolate, &details, false).await?;
    let name = input["database"].as_str().unwrap_or("FixtureDatabase");
    let provider = hello
        .providers
        .iter()
        .find(|p| p.kind == "drift" && p.name == name)
        .ok_or("Fixture database not found")?;
    let result = native
        .call(
            "drift.query",
            json!({"provider":provider.id,"sql":input["sql"]}),
        )
        .await?;
    println!("{result}");
    Ok(())
}
