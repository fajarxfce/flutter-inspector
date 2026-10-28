use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub view: bool,
}
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub kind: String,
    pub pk: bool,
    pub generated: bool,
    pub default: String,
    pub required: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Request {
    Databases,
    Tables,
    Rows,
    Save,
    Delete,
    Empty,
    Query,
    Execute,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Stored(usize),
    New(u64),
}
pub struct Cell {
    pub row: Row,
    pub column: usize,
    pub text: String,
    pub original_text: String,
    pub touched: bool,
    pub focus: bool,
}
#[derive(Clone, Copy)]
pub enum Confirmation {
    Delete,
    Empty,
}
pub struct DriftEditor {
    pub id: String,
    pub tables: Vec<Table>,
    pub table: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Value>,
    pub total: u64,
    pub table_filter: String,
    pub filter: String,
    pub offset: usize,
    pub order: String,
    pub descending: bool,
    pub edits: BTreeMap<usize, Map<String, Value>>,
    pub inserts: BTreeMap<u64, Map<String, Value>>,
    pub selected: BTreeSet<usize>,
    pub cell: Option<Cell>,
    pub confirmation: Option<Confirmation>,
    pub error: Option<String>,
    pub sql: String,
    pub query_result: Value,
    pub show_sql: bool,
    requests: HashMap<Request, String>,
    next_row: u64,
}
impl Default for DriftEditor {
    fn default() -> Self {
        Self {
            id: String::new(),
            tables: vec![],
            table: String::new(),
            columns: vec![],
            rows: vec![],
            total: 0,
            table_filter: String::new(),
            filter: String::new(),
            offset: 0,
            order: String::new(),
            descending: false,
            edits: BTreeMap::new(),
            inserts: BTreeMap::new(),
            selected: BTreeSet::new(),
            cell: None,
            confirmation: None,
            error: None,
            sql: "SELECT name, type FROM sqlite_master".into(),
            query_result: Value::Null,
            show_sql: false,
            requests: HashMap::new(),
            next_row: 0,
        }
    }
}
impl DriftEditor {
    pub fn database(&mut self, id: String) {
        // No response or editable data from a previous database survives a switch.
        *self = Self {
            id,
            ..Self::default()
        };
    }
    pub fn select_table(&mut self, name: String) {
        self.requests.remove(&Request::Rows);
        self.table = name;
        self.columns.clear();
        self.rows.clear();
        self.total = 0;
        self.offset = 0;
        self.order.clear();
        self.filter.clear();
        self.descending = false;
        self.discard();
        self.selected.clear();
        self.error = None;
    }
    pub fn begin(&mut self, kind: Request) -> String {
        let tag = format!("drift.{kind:?}.{}", uuid::Uuid::new_v4());
        self.requests.insert(kind, tag.clone());
        tag
    }
    pub fn finish(&mut self, tag: &str) -> Option<Request> {
        let kind = self
            .requests
            .iter()
            .find(|(_, value)| value.as_str() == tag)
            .map(|(kind, _)| *kind)?;
        self.requests.remove(&kind);
        Some(kind)
    }
    pub fn invalidate(&mut self) {
        self.requests.clear();
        self.confirmation = None;
    }
    pub fn busy(&self) -> bool {
        [
            Request::Save,
            Request::Delete,
            Request::Empty,
            Request::Execute,
        ]
        .iter()
        .any(|k| self.requests.contains_key(k))
    }
    pub fn loading(&self) -> bool {
        self.requests.contains_key(&Request::Databases)
            || self.requests.contains_key(&Request::Rows)
            || self.requests.contains_key(&Request::Tables)
    }
    pub fn pending(&self) -> bool {
        !self.edits.is_empty() || !self.inserts.is_empty() || self.cell.is_some()
    }
    pub fn writable(&self) -> bool {
        self.tables.iter().any(|t| t.name == self.table && !t.view)
    }
    pub fn accept_tables(&mut self, value: Value) {
        self.tables = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(Table {
                    name: v["name"].as_str()?.into(),
                    view: v["type"] == "view",
                })
            })
            .collect();
        if !self.tables.iter().any(|t| t.name == self.table) {
            self.select_table(
                self.tables
                    .first()
                    .map(|t| t.name.clone())
                    .unwrap_or_default(),
            );
        }
    }
    pub fn accept_rows(&mut self, mut value: Value) {
        self.columns = value["columns"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(Column {
                    name: v["name"].as_str()?.into(),
                    kind: v["type"].as_str().unwrap_or("").into(),
                    pk: v["pk"].as_i64().unwrap_or(0) > 0,
                    generated: v["hidden"].as_i64().unwrap_or(0) != 0,
                    default: v["dflt_value"].as_str().unwrap_or("").into(),
                    required: v["notnull"] == 1,
                })
            })
            .collect();
        self.rows = match value.get_mut("rows").map(Value::take) {
            Some(Value::Array(rows)) => rows,
            _ => vec![],
        };
        self.total = value["total"].as_u64().unwrap_or(0);
        self.selected.clear();
    }
    pub fn identity(&self, index: usize) -> Option<Value> {
        let row = self.rows.get(index)?;
        let mut identity = Map::new();
        for column in self.columns.iter().filter(|c| c.pk) {
            let value = row.get(&column.name).filter(|v| !v.is_null())?;
            identity.insert(column.name.clone(), value.clone());
        }
        (!identity.is_empty()).then_some(Value::Object(identity))
    }
    pub fn has_identity(&self, index: usize) -> bool {
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        let mut has_key = false;
        for column in self.columns.iter().filter(|c| c.pk) {
            has_key = true;
            if row.get(&column.name).is_none_or(Value::is_null) {
                return false;
            }
        }
        has_key
    }
    pub fn value(&self, row: Row, name: &str) -> Option<&Value> {
        match row {
            Row::Stored(index) => self
                .edits
                .get(&index)
                .and_then(|v| v.get(name))
                .or_else(|| self.rows.get(index)?.get(name)),
            Row::New(id) => self.inserts.get(&id)?.get(name),
        }
    }
    pub fn set(&mut self, row: Row, name: &str, value: Option<Value>) {
        match row {
            Row::New(id) => {
                if let Some(values) = self.inserts.get_mut(&id) {
                    if let Some(value) = value {
                        values.insert(name.into(), value);
                    } else {
                        values.remove(name);
                    }
                }
            }
            Row::Stored(index) => {
                let values = self.edits.entry(index).or_default();
                if let Some(value) = value.filter(|v| self.rows[index].get(name) != Some(v)) {
                    values.insert(name.into(), value);
                } else {
                    values.remove(name);
                }
                if values.is_empty() {
                    self.edits.remove(&index);
                }
            }
        }
    }
    pub fn edit(&mut self, row: Row, column: usize) -> Result<(), String> {
        self.commit_cell()?;
        let meta = self.columns.get(column).ok_or("Column unavailable")?;
        if meta.generated || (matches!(row,Row::Stored(i) if !self.has_identity(i))) {
            return Err("This row or column is read-only".into());
        }
        let text = self
            .value(row, &meta.name)
            .map(edit_text)
            .unwrap_or_default();
        if text.len() > 256 * 1024 {
            return Err("Cells larger than 256 KiB can be read or changed through SQL".into());
        }
        self.cell = Some(Cell {
            row,
            column,
            original_text: text.clone(),
            text,
            touched: false,
            focus: true,
        });
        Ok(())
    }
    pub fn commit_cell(&mut self) -> Result<(), String> {
        if let Some(cell) = &self.cell {
            if !cell.touched && cell.text == cell.original_text {
                self.cell = None;
                return Ok(());
            }
            let column = &self.columns[cell.column];
            let value = parse_value(column, &cell.text)?;
            let row = cell.row;
            let name = column.name.clone();
            self.set(row, &name, Some(value));
            self.cell = None;
        }
        Ok(())
    }
    pub fn new_row(&mut self) -> Result<(), String> {
        self.commit_cell()?;
        if self.inserts.len() + self.edits.len() >= 200 {
            return Err("Save the current batch before adding more rows".into());
        }
        let id = self.next_row;
        self.next_row += 1;
        self.inserts.insert(id, Map::new());
        if let Some(column) = self.columns.iter().position(|c| !c.generated && !c.pk) {
            self.edit(Row::New(id), column)?;
        }
        Ok(())
    }
    pub fn changes(&self) -> Result<Value, String> {
        let mut changes = Vec::new();
        for (index, values) in &self.edits {
            let identity = self
                .identity(*index)
                .ok_or("Row has no complete primary key")?;
            let original: Map<String, Value> = values
                .keys()
                .map(|k| (k.clone(), self.rows[*index][k].clone()))
                .collect();
            changes.push(
                json!({"kind":"update","identity":identity,"values":values,"original":original}),
            );
        }
        changes.extend(
            self.inserts
                .values()
                .map(|v| json!({"kind":"insert","values":v})),
        );
        Ok(changes.into())
    }
    pub fn deletes(&self) -> Result<Value, String> {
        self.selected
            .iter()
            .map(|index| {
                self.identity(*index)
                    .map(|identity| json!({"kind":"delete","identity":identity}))
                    .ok_or("Row has no complete primary key".into())
            })
            .collect::<Result<Vec<Value>, String>>()
            .map(Value::Array)
    }
    pub fn discard(&mut self) {
        self.edits.clear();
        self.inserts.clear();
        self.cell = None;
        self.confirmation = None;
    }
}
pub fn edit_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.into()
    } else if let Some(blob) = value["base64"].as_str() {
        blob.into()
    } else if value.is_null() {
        String::new()
    } else {
        value.to_string()
    }
}
pub fn preview(value: Option<&Value>) -> String {
    match value {
        None => "DEFAULT".into(),
        Some(Value::Null) => "NULL".into(),
        Some(Value::String(text)) => text.chars().take(150).collect(),
        Some(v) if v["base64"].is_string() => format!(
            "BLOB · ~{} bytes",
            v["base64"].as_str().unwrap().len() * 3 / 4
        ),
        Some(v) => v.to_string(),
    }
}
fn parse_value(column: &Column, text: &str) -> Result<Value, String> {
    let kind = column.kind.to_ascii_uppercase();
    if kind.contains("INT") {
        return text
            .trim()
            .parse::<i64>()
            .map(|v| json!(v))
            .map_err(|_| format!("{} needs a whole number", column.name));
    }
    if ["REAL", "FLOA", "DOUB"].iter().any(|k| kind.contains(k)) {
        return text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or(format!("{} needs a finite number", column.name));
    }
    if kind.contains("BLOB") {
        STANDARD
            .decode(text.trim())
            .map_err(|_| format!("{} needs base64 bytes", column.name))?;
        return Ok(json!({"base64":text.trim()}));
    }
    if kind.contains("BOOL") {
        return match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(json!(1)),
            "false" | "0" => Ok(json!(0)),
            _ => Err("Enter true/false or 1/0".into()),
        };
    }
    Ok(Value::String(text.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn database_switch_clears_data_and_rejects_late_responses() {
        let mut editor = DriftEditor::default();
        editor.database("a".into());
        let old_tables = editor.begin(Request::Tables);
        editor.accept_tables(json!([{"name":"old_table","type":"table"}]));
        let old_rows = editor.begin(Request::Rows);
        editor.rows = vec![json!({"secret":"a"})];
        editor.query_result = json!([{"old":"result"}]);
        editor.database("b".into());
        let new_tables = editor.begin(Request::Tables);
        assert!(editor.tables.is_empty() && editor.rows.is_empty() && editor.columns.is_empty());
        assert!(editor.query_result.is_null());
        assert_eq!(editor.finish(&old_tables), None);
        assert_eq!(editor.finish(&old_rows), None);
        assert_eq!(editor.finish(&new_tables), Some(Request::Tables));
        editor.accept_tables(json!([{"name":"b_table","type":"table"}]));
        assert_eq!(editor.table, "b_table");
    }
    #[test]
    fn editing_keeps_original_composite_key_and_only_changed_cells() {
        let mut editor = DriftEditor::default();
        editor.accept_rows(json!({"columns":[{"name":"tenant","type":"TEXT","pk":1},{"name":"id","type":"INTEGER","pk":2},{"name":"name","type":"TEXT"}],"rows":[{"tenant":"a","id":4,"name":"before"}],"total":1}));
        editor.edit(Row::Stored(0), 1).unwrap();
        editor.cell.as_mut().unwrap().text = "9".into();
        editor.commit_cell().unwrap();
        editor.set(Row::Stored(0), "name", Some(json!("after")));
        let changes = editor.changes().unwrap();
        assert_eq!(changes[0]["identity"], json!({"tenant":"a","id":4}));
        assert_eq!(changes[0]["original"], json!({"id":4,"name":"before"}));
        assert_eq!(changes[0]["values"], json!({"id":9,"name":"after"}));
        editor.set(Row::Stored(0), "id", Some(json!(4)));
        assert_eq!(editor.edits[&0].len(), 1);
    }
    #[test]
    fn table_switch_and_refresh_reject_out_of_order_rows() {
        let mut editor = DriftEditor::default();
        editor.select_table("a".into());
        let old = editor.begin(Request::Rows);
        editor.select_table("b".into());
        let first = editor.begin(Request::Rows);
        let latest = editor.begin(Request::Rows);
        assert_eq!(editor.finish(&old), None);
        assert_eq!(editor.finish(&first), None);
        assert!(editor.loading());
        assert_eq!(editor.finish(&latest), Some(Request::Rows));
        assert!(!editor.loading());
    }
    #[test]
    fn previews_bound_large_cells_and_defaults_remain_omitted() {
        let text = Value::String("🙂".repeat(1_000_000));
        assert_eq!(preview(Some(&text)).chars().count(), 150);
        let mut editor = DriftEditor::default();
        editor.inserts.insert(0, Map::new());
        editor.set(Row::New(0), "optional", Some(Value::Null));
        assert_eq!(
            editor.changes().unwrap()[0]["values"],
            json!({"optional":null})
        );
        editor.set(Row::New(0), "optional", None);
        assert_eq!(editor.changes().unwrap()[0]["values"], json!({}));
    }
    #[test]
    fn visiting_a_cell_preserves_null_and_insert_defaults() {
        let mut editor = DriftEditor::default();
        editor.accept_rows(json!({"columns":[{"name":"id","type":"INTEGER","pk":1},{"name":"optional","type":"TEXT","dflt_value":"'fallback'"}],"rows":[{"id":1,"optional":null}],"total":1}));
        editor.edit(Row::Stored(0), 1).unwrap();
        editor.commit_cell().unwrap();
        assert!(!editor.pending());
        assert_eq!(editor.rows[0]["optional"], Value::Null);
        editor.new_row().unwrap();
        editor.commit_cell().unwrap();
        assert_eq!(editor.changes().unwrap()[0]["values"], json!({}));
        editor.edit(Row::New(0), 1).unwrap();
        editor.cell.as_mut().unwrap().touched = true;
        editor.commit_cell().unwrap();
        assert_eq!(
            editor.changes().unwrap()[0]["values"],
            json!({"optional":""})
        );
    }
}
