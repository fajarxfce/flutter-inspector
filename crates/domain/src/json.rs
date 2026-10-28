use serde_json::Value;
#[derive(Clone, Debug)]
pub struct JsonNode {
    pub pointer: String,
    pub label: String,
    pub summary: String,
    pub depth: usize,
    pub children: Vec<usize>,
}
#[derive(Clone, Debug)]
pub struct JsonDocument {
    pub value: Value,
    pub nodes: Vec<JsonNode>,
    pub limited: bool,
}
impl JsonDocument {
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(text)?;
        let mut nodes = Vec::new();
        Self::build(&value, String::new(), "$".into(), 0, &mut nodes);
        let limited = nodes.len() >= 100_000;
        Ok(Self {
            value,
            nodes,
            limited,
        })
    }
    fn build(
        value: &Value,
        pointer: String,
        label: String,
        depth: usize,
        nodes: &mut Vec<JsonNode>,
    ) -> usize {
        let index = nodes.len();
        let summary = match value {
            Value::Object(v) => format!("{{ {} keys }}", v.len()),
            Value::Array(v) => format!("[ {} items ]", v.len()),
            _ => {
                let s = value.to_string();
                if s.chars().count() > 240 {
                    s.chars().take(240).collect::<String>() + "…"
                } else {
                    s
                }
            }
        };
        nodes.push(JsonNode {
            pointer: pointer.clone(),
            label,
            summary,
            depth,
            children: vec![],
        });
        let mut children = vec![];
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if nodes.len() >= 100_000 {
                        break;
                    }
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    children.push(Self::build(
                        value,
                        format!("{pointer}/{escaped}"),
                        key.clone(),
                        depth + 1,
                        nodes,
                    ));
                }
            }
            Value::Array(array) => {
                for (i, value) in array.iter().enumerate() {
                    if nodes.len() >= 100_000 {
                        break;
                    }
                    children.push(Self::build(
                        value,
                        format!("{pointer}/{i}"),
                        i.to_string(),
                        depth + 1,
                        nodes,
                    ));
                }
            }
            _ => {}
        }
        nodes[index].children = children;
        index
    }
    pub fn visible(&self, expanded: &std::collections::HashSet<usize>) -> Vec<usize> {
        let mut visible = vec![];
        let mut stack = vec![0];
        while let Some(i) = stack.pop() {
            visible.push(i);
            if expanded.contains(&i) {
                stack.extend(self.nodes[i].children.iter().rev());
            }
        }
        visible
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointers_and_large_numbers() {
        let doc =
            JsonDocument::parse(r#"{"a/b":{"~":9007199254740993},"empty":[]}"#).expect("json");
        assert_eq!(
            doc.value.pointer("/a~1b/~0").map(Value::to_string),
            Some("9007199254740993".into())
        );
        assert_eq!(doc.visible(&Default::default()), vec![0]);
    }
}
