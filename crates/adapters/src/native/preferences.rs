use super::dart_string;
use crate::vm::VmClient;
use serde_json::Value;

pub(super) async fn call(
    vm: &VmClient,
    isolate: &str,
    library: &str,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let legacy = match params["provider"].as_str() {
        Some("native.preferences.legacy") => true,
        Some("native.preferences.async") => false,
        _ => return Err("Select a detected preferences store".into()),
    };
    let setup = if legacy {
        "final prefs = await SharedPreferences.getInstance(); await prefs.reload();"
    } else {
        "final prefs = SharedPreferencesAsync();"
    };
    let operation = match method {
        "preferences.list" => {
            let read = if legacy {
                "{for (final key in prefs.getKeys()) key: prefs.get(key)}"
            } else {
                "await prefs.getAll()"
            };
            format!(
                r#"
                final values = {read};
                return (values.entries.map((e) => {{'key':e.key, 'value':e.value,
                  'type': e.value is List ? 'List<String>' : e.value.runtimeType.toString()}}).toList()
                  ..sort((a,b) => (a['key'] as String).compareTo(b['key'] as String)));
            "#
            )
        }
        "preferences.remove" => format!(
            "await prefs.remove({}); return null;",
            dart_string(params["key"].as_str().ok_or("Missing preference key")?)
        ),
        "preferences.set" => {
            let value = &params["value"];
            let (setter, conversion) = match params["type"].as_str() {
                Some("String") if value.is_string() => ("setString", " as String"),
                Some("int") if value.as_i64().is_some() => ("setInt", " as int"),
                Some("double") if value.is_number() => ("setDouble", ".toDouble()"),
                Some("bool") if value.is_boolean() => ("setBool", " as bool"),
                Some("List<String>")
                    if value
                        .as_array()
                        .is_some_and(|a| a.iter().all(Value::is_string)) =>
                {
                    ("setStringList", ".cast<String>()")
                }
                _ => return Err("Preference value does not match its selected type".into()),
            };
            let key = dart_string(params["key"].as_str().ok_or("Missing preference key")?);
            let encoded = dart_string(&value.to_string());
            format!(
                "final value = jsonDecode({encoded}); await prefs.{setter}({key}, value{conversion}); return null;"
            )
        }
        _ => return Err("Unsupported preferences operation".into()),
    };
    vm.evaluate_async(isolate, library, &format!("{setup}\n{operation}"))
        .await
}
