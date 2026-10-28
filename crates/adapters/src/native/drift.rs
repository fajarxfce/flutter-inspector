use crate::vm::VmClient;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};

pub(super) struct Database<'a> {
    vm: &'a VmClient,
    isolate: &'a str,
    id: i64,
}
impl<'a> Database<'a> {
    pub fn new(vm: &'a VmClient, isolate: &'a str, id: i64) -> Self {
        Self { vm, isolate, id }
    }
    async fn extension(&self, action: &str, mut params: Value) -> Result<Value, String> {
        params["isolateId"] = self.isolate.into();
        params["db"] = self.id.to_string().into();
        params["action"] = action.into();
        let result = self.vm.call("ext.drift.database", params).await?;
        result
            .get("r")
            .cloned()
            .ok_or("Unsupported Drift DevTools protocol".into())
    }
    async fn query(&self, sql: &str, args: Vec<Value>, select: bool) -> Result<Value, String> {
        let args = args
            .into_iter()
            .map(encode)
            .collect::<Result<Vec<_>, _>>()?;
        // DriftProtocol ExecuteQuery tag 3, StatementMethod select=3/custom=0.
        let query = json!([3, if select { 3 } else { 0 }, sql, args, null]).to_string();
        let result = self
            .extension("execute-query", json!({"query":query}))
            .await?;
        if select {
            decode_rows(result)
        } else {
            Ok(result)
        }
    }
    async fn columns(&self, table: &str) -> Result<Vec<Value>, String> {
        let exists = self
            .query(
                "SELECT type FROM sqlite_master WHERE name=? AND type IN ('table','view')",
                vec![table.into()],
                true,
            )
            .await?;
        if exists.as_array().is_none_or(Vec::is_empty) {
            return Err("Unknown table or view".into());
        }
        let result = self
            .query(
                &format!("PRAGMA table_xinfo({})", identifier(table)),
                vec![],
                true,
            )
            .await?;
        result
            .as_array()
            .cloned()
            .ok_or("Invalid table schema".into())
    }
    async fn notify(&self, tables: Vec<Value>) -> Result<(), String> {
        self.extension(
            "notify-update",
            json!({"updates":serde_json::to_string(&tables).map_err(|e| e.to_string())?}),
        )
        .await?;
        Ok(())
    }
    pub async fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        if method == "drift.tables" {
            return self.query("SELECT name,type,sql FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name", vec![], true).await;
        }
        if method == "drift.query" || method == "drift.execute" {
            let sql = single_statement(params["sql"].as_str().ok_or("Missing SQL")?)?;
            if method == "drift.query" {
                let word = sql.split_whitespace().next().unwrap_or_default();
                if !word.eq_ignore_ascii_case("select") && !word.eq_ignore_ascii_case("with") {
                    return Err(
                        "Query accepts SELECT/WITH; use Execute for other statements".into(),
                    );
                }
                return self
                    .query(&format!("SELECT * FROM ({sql}) LIMIT 1000"), vec![], true)
                    .await;
            }
            self.query(&sql, vec![], false).await?;
            let tables = self
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table'",
                    vec![],
                    true,
                )
                .await?;
            self.notify(
                tables
                    .as_array()
                    .ok_or("Invalid tables")?
                    .iter()
                    .map(|r| json!({"table":r["name"]}))
                    .collect(),
            )
            .await?;
            return Ok(json!({"executed":true}));
        }
        let table = params["table"].as_str().ok_or("Missing table")?;
        let columns = self.columns(table).await?;
        if method == "drift.columns" {
            return Ok(columns.into());
        }
        let names: Vec<&str> = columns.iter().filter_map(|c| c["name"].as_str()).collect();
        let target = identifier(table);
        if method == "drift.rows" {
            let offset = params["offset"].as_u64().unwrap_or(0).min(i64::MAX as u64);
            let filter = params["filter"].as_str().unwrap_or_default();
            let order = params["order"].as_str().unwrap_or_default();
            if !order.is_empty() && !names.contains(&order) {
                return Err("Unknown sort column".into());
            }
            let mut args = Vec::new();
            let condition = if filter.is_empty() {
                String::new()
            } else {
                let expressions: Vec<_> = names
                    .iter()
                    .map(|n| {
                        args.push(json!(format!("%{filter}%")));
                        format!("CAST({} AS TEXT) LIKE ?", identifier(n))
                    })
                    .collect();
                format!(" WHERE {}", expressions.join(" OR "))
            };
            let count = self
                .query(
                    &format!("SELECT COUNT(*) AS total FROM {target}{condition}"),
                    args.clone(),
                    true,
                )
                .await?;
            let sorting = if order.is_empty() {
                String::new()
            } else {
                format!(
                    " ORDER BY {} {}",
                    identifier(order),
                    if params["descending"] == true {
                        "DESC"
                    } else {
                        "ASC"
                    }
                )
            };
            args.push(json!(offset));
            let rows = self
                .query(
                    &format!("SELECT * FROM {target}{condition}{sorting} LIMIT 200 OFFSET ?"),
                    args,
                    true,
                )
                .await?;
            return Ok(json!({"columns":columns,"rows":rows,"total":count[0]["total"]}));
        }
        let kind = self
            .query(
                "SELECT type FROM sqlite_master WHERE name=?",
                vec![table.into()],
                true,
            )
            .await?;
        if kind[0]["type"] != "table" {
            return Err("Views are read-only".into());
        }
        if matches!(method, "drift.apply" | "drift.empty") {
            let commands = if method == "drift.empty" {
                if params["confirm"] != table {
                    return Err("Confirm the table name before emptying it".into());
                }
                vec![json!({"sql":format!("DELETE FROM {target}"),"args":[],"single":false})]
            } else {
                batch_commands(table, &columns, &params["changes"])?
            };
            let details = self
                .vm
                .call("getIsolate", json!({"isolateId":self.isolate}))
                .await?;
            let library =
                super::library(&details, super::DRIFT).ok_or("Drift debug library unavailable")?;
            let commands =
                super::dart_string(&serde_json::to_string(&commands).map_err(|e| e.to_string())?);
            let table_literal = super::dart_string(table);
            let statements = format!(
                r#"
                final db = TrackedDatabase.all.firstWhere((d) => d.id == {}).database;
                final commands = jsonDecode({commands});
                var affected = 0;
                await db.transaction(() async {{
                  for (final c in commands) {{
                    final args = [for (final v in c['args']) v is Map ? base64Decode(v['base64'] as String) : v is bool ? (v ? 1 : 0) : v];
                    await db.customStatement(c['sql'] as String, args);
                    final result = await db.customSelect('SELECT changes() AS n').getSingle();
                    final count = result.data['n'] as int;
                    if (c['single'] == true && count != 1) throw StateError('Row changed or disappeared. Refresh before trying again.');
                    affected += count;
                  }}
                  db.notifyUpdates({{TableUpdate({table_literal})}});
                }});
                return {{'changed': affected}};
            "#,
                self.id
            );
            return self
                .vm
                .evaluate_async(self.isolate, &library, &statements)
                .await;
        }
        let empty = Map::new();
        let values = params["values"].as_object().unwrap_or(&empty);
        if values.keys().any(|name| !names.contains(&name.as_str())) {
            return Err("Unknown column".into());
        }
        let mut args: Vec<Value> = values.values().cloned().collect();
        let sql = if method == "drift.insert" {
            if values.is_empty() {
                return Err("Provide values to insert".into());
            }
            format!(
                "INSERT INTO {target} ({}) VALUES ({})",
                values
                    .keys()
                    .map(|n| identifier(n))
                    .collect::<Vec<_>>()
                    .join(","),
                vec!["?"; values.len()].join(",")
            )
        } else {
            let keys: Vec<&str> = columns
                .iter()
                .filter(|c| c["pk"].as_i64().unwrap_or(0) > 0)
                .filter_map(|c| c["name"].as_str())
                .collect();
            let identity = params["identity"]
                .as_object()
                .ok_or("A complete primary key is required")?;
            if keys.is_empty() || keys.iter().any(|k| !identity.contains_key(*k)) {
                return Err("A complete primary key is required".into());
            }
            let condition = keys
                .iter()
                .map(|k| format!("{} IS ?", identifier(k)))
                .collect::<Vec<_>>()
                .join(" AND ");
            match method {
                "drift.update" => {
                    if values.is_empty() {
                        return Err("Provide values to update".into());
                    }
                    args.extend(keys.iter().map(|k| identity[*k].clone()));
                    format!(
                        "UPDATE {target} SET {} WHERE {condition}",
                        values
                            .keys()
                            .map(|n| format!("{}=?", identifier(n)))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }
                "drift.delete" => {
                    args = keys.iter().map(|k| identity[*k].clone()).collect();
                    format!("DELETE FROM {target} WHERE {condition}")
                }
                _ => return Err("Unsupported Drift operation".into()),
            }
        };
        self.query(&sql, args, false).await?;
        self.notify(vec![json!({"table":table})]).await?;
        Ok(json!({"changed":true}))
    }
}
/// Compile a bounded batch. All identifiers come from verified schema; values
/// remain parameters. Expected values detect edits to the same cells by the app.
fn batch_commands(table: &str, columns: &[Value], changes: &Value) -> Result<Vec<Value>, String> {
    let changes = changes.as_array().ok_or("Changes must be a list")?;
    if changes.is_empty() || changes.len() > 200 {
        return Err("Save between 1 and 200 rows at a time".into());
    }
    if serde_json::to_vec(changes)
        .map_err(|e| e.to_string())?
        .len()
        > 2 * 1024 * 1024
    {
        return Err("Changes exceed 2 MiB".into());
    }
    let names: Vec<_> = columns
        .iter()
        .filter(|c| c["hidden"].as_i64().unwrap_or(0) == 0)
        .filter_map(|c| c["name"].as_str())
        .collect();
    let keys: Vec<_> = columns
        .iter()
        .filter(|c| c["pk"].as_i64().unwrap_or(0) > 0)
        .filter_map(|c| c["name"].as_str())
        .collect();
    let target = identifier(table);
    let mut result = Vec::new();
    for change in changes {
        let empty = Map::new();
        let values = change["values"].as_object().unwrap_or(&empty);
        let original = change["original"].as_object().unwrap_or(&empty);
        if values
            .keys()
            .chain(original.keys())
            .any(|n| !names.contains(&n.as_str()))
        {
            return Err("Unknown or generated column".into());
        }
        for value in values.values().chain(original.values()) {
            encode(value.clone())?;
        }
        let mut args: Vec<Value> = values.values().cloned().collect();
        let kind = change["kind"].as_str().ok_or("Missing change kind")?;
        let sql = if kind == "insert" {
            if values.is_empty() {
                format!("INSERT INTO {target} DEFAULT VALUES")
            } else {
                format!(
                    "INSERT INTO {target} ({}) VALUES ({})",
                    values
                        .keys()
                        .map(|n| identifier(n))
                        .collect::<Vec<_>>()
                        .join(","),
                    vec!["?"; values.len()].join(",")
                )
            }
        } else {
            let identity = change["identity"]
                .as_object()
                .ok_or("A complete primary key is required")?;
            if keys.is_empty()
                || keys
                    .iter()
                    .any(|k| identity.get(*k).is_none_or(Value::is_null))
            {
                return Err("A complete non-null primary key is required".into());
            }
            let mut conditions = Vec::new();
            let mut conditions_args = Vec::new();
            for (key, value) in keys
                .iter()
                .map(|k| (*k, &identity[*k]))
                .chain(original.iter().map(|(k, v)| (k.as_str(), v)))
            {
                encode(value.clone())?;
                conditions.push(format!("{} IS ?", identifier(key)));
                conditions_args.push(value.clone());
            }
            let condition = conditions.join(" AND ");
            match kind {
                "update" => {
                    if values.is_empty() {
                        return Err("Provide values to update".into());
                    }
                    args.extend(conditions_args);
                    format!(
                        "UPDATE {target} SET {} WHERE {condition}",
                        values
                            .keys()
                            .map(|n| format!("{}=?", identifier(n)))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }
                "delete" => {
                    args = conditions_args;
                    format!("DELETE FROM {target} WHERE {condition}")
                }
                _ => return Err("Unsupported change kind".into()),
            }
        };
        result.push(json!({"sql":sql,"args":args,"single":kind != "insert"}));
    }
    Ok(result)
}
fn identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn encode(value: Value) -> Result<Value, String> {
    if let Some(object) = value.as_object() {
        if object.len() == 1
            && let Some(bytes) = object.get("base64").and_then(Value::as_str)
        {
            return Ok(json!(STANDARD.decode(bytes).map_err(|e| e.to_string())?));
        }
        return Err("SQL values must be scalars or {\"base64\":\"…\"}".into());
    }
    if value.is_array() {
        return Err("Use base64 for BLOB values".into());
    }
    Ok(value)
}
fn decode_rows(value: Value) -> Result<Value, String> {
    let values = value.as_array().ok_or("Invalid Drift result")?;
    if values.first() != Some(&json!(11)) {
        return Err("Unsupported Drift result protocol".into());
    }
    if values.len() == 1 {
        return Ok(json!([]));
    }
    let count = values
        .get(1)
        .and_then(Value::as_u64)
        .ok_or("Missing Drift column count")? as usize;
    if count > 2000 || values.len() < count + 3 {
        return Err("Invalid Drift columns".into());
    }
    let rows = values[count + 2]
        .as_u64()
        .ok_or("Missing Drift row count")? as usize;
    if rows > 10000 || values.len() != 3 + count + count * rows {
        return Err("Invalid or oversized Drift result".into());
    }
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut object = Map::new();
        for col in 0..count {
            let name = values[col + 2].as_str().ok_or("Invalid Drift column")?;
            let value = &values[3 + count + row * count + col];
            let value = if let Some(bytes) = value.as_array() {
                let bytes = bytes
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .filter(|v| *v < 256)
                            .map(|v| v as u8)
                            .ok_or("Unsupported Drift BLOB value")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                json!({"base64":STANDARD.encode(bytes)})
            } else {
                value.clone()
            };
            object.insert(name.into(), value);
        }
        result.push(Value::Object(object));
    }
    Ok(result.into())
}

/// Remove comments and an optional trailing terminator, rejecting stacked SQL.
fn single_statement(input: &str) -> Result<String, String> {
    if input.len() > 65536 {
        return Err("SQL exceeds 64 KiB".into());
    }
    let mut chars = input.chars().peekable();
    let mut output = String::new();
    let mut ended = false;
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek() == Some(&'-') {
            chars.next();
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
            output.push(' ');
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err("Unterminated SQL comment".into());
            }
            output.push(' ');
            continue;
        }
        if ended {
            if !c.is_whitespace() {
                return Err("Only one SQL statement is allowed".into());
            }
            continue;
        }
        if c == ';' {
            ended = true;
            continue;
        }
        output.push(c);
        if matches!(c, '\'' | '"' | '`' | '[') {
            let close = if c == '[' { ']' } else { c };
            let mut closed = false;
            while let Some(c) = chars.next() {
                output.push(c);
                if c == close {
                    if chars.peek() == Some(&close) && close != ']' {
                        output.push(chars.next().unwrap());
                    } else {
                        closed = true;
                        break;
                    }
                }
            }
            if !closed {
                return Err("Unterminated SQL quote".into());
            }
        }
    }
    let sql = output.trim();
    if sql.is_empty() {
        return Err("SQL is empty".into());
    }
    Ok(sql.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sql_literals_and_comments_are_preserved_without_stacked_statements() {
        assert_eq!(
            single_statement("/*hi*/ SELECT ';''x' AS \"semi;colon\"; --end").unwrap(),
            "SELECT ';''x' AS \"semi;colon\""
        );
        assert!(single_statement("SELECT 1; DELETE FROM notes").is_err());
        assert!(single_statement("SELECT 'unfinished").is_err());
    }
    #[test]
    fn wire_rows_preserve_blob_null_and_64_bit_integer() {
        let result = decode_rows(json!([
            11,
            3,
            "blob",
            "nullable",
            "large",
            1,
            [0, 255],
            null,
            9007199254740993i64
        ]))
        .unwrap();
        assert_eq!(
            result[0],
            json!({"blob":{"base64":"AP8="},"nullable":null,"large":9007199254740993i64})
        );
        assert!(decode_rows(json!([11, 3])).is_err());
    }
}
