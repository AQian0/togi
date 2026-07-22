use crate::shared::util::parse_args_object;
use rig::tool::{ToolCallExtensions, ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use serde_json::{Map, Value};
use std::fmt::Write;

pub const OFFSET_PARAM: &str = "offset";
pub const LIMIT_PARAM: &str = "limit";

pub fn paginate(default_limit: usize, tools: Vec<Box<dyn ToolDyn>>) -> Vec<Box<dyn ToolDyn>> {
    tools
        .into_iter()
        .map(|tool| {
            Box::new(PaginatedTool {
                inner: tool,
                default_limit,
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

struct PaginatedTool {
    inner: Box<dyn ToolDyn>,
    default_limit: usize,
}

impl ToolDyn for PaginatedTool {
    fn name(&self) -> String {
        self.inner.name()
    }
    fn description(&self) -> String {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        let mut parameters = self.inner.parameters();
        add_pagination_params(&mut parameters);
        parameters
    }
    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            self.call_with_extensions(args, &ToolCallExtensions::new())
                .await
        })
    }

    fn call_with_extensions<'a>(
        &'a self,
        args: String,
        extensions: &'a ToolCallExtensions,
    ) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let mut args = parse_args_object(&args)?;
            let offset = take_usize(&mut args, OFFSET_PARAM)?;
            if offset == Some(0) {
                return Err(ToolError::ToolCallError(
                    crate::t!("paginate-offset-one-based").into(),
                ));
            }
            let limit = take_usize(&mut args, LIMIT_PARAM)?;
            let inner_args = serde_json::to_string(&args).map_err(ToolError::JsonError)?;
            let output = self.inner.call_with_extensions(inner_args, extensions).await?;
            Ok(paginate_text(&output, offset, limit, self.default_limit))
        })
    }
}


fn paginate_text(
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    default_limit: usize,
) -> String {
    let total = text.lines().count();
    if total == 0 {
        return text.to_string();
    }
    let start = offset.unwrap_or(1).max(1);
    if start > total {
        return crate::t!("paginate-past-end", offset = start, total = total);
    }
    let start_idx = start - 1;
    let effective_limit = match limit {
        Some(0) => 0,
        Some(n) => n,
        None => default_limit,
    };
    let end_idx = if effective_limit == 0 {
        total
    } else {
        (start_idx + effective_limit).min(total)
    };
    let paginated = start_idx > 0 || end_idx < total;
    if !paginated {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len().min(16_384) + 96);
    let _ = writeln!(
        out,
        "{}",
        crate::t!(
            "paginate-showing",
            start = start_idx + 1,
            end = end_idx,
            total = total
        )
    );
    for line in text.lines().skip(start_idx).take(end_idx - start_idx) {
        out.push_str(line);
        out.push('\n');
    }
    if end_idx < total {
        let _ = writeln!(
            out,
            "{}",
            crate::t!(
                "paginate-more-lines",
                count = total - end_idx,
                offset = end_idx + 1
            ),
        );
    }
    out
}

fn add_pagination_params(parameters: &mut Value) {
    let Some(schema) = parameters.as_object_mut() else {
        return;
    };
    let properties = schema
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(properties) = properties.as_object_mut() else {
        return;
    };
    // ponytail: inline the two simple params instead of schemars derive round-trip
    properties.insert(
        OFFSET_PARAM.to_string(),
        serde_json::json!({
            "type": "integer",
            "minimum": 1,
            "description": "1-based line number to start from. Use with `limit` to page through large output."
        }),
    );
    properties.insert(
        LIMIT_PARAM.to_string(),
        serde_json::json!({
            "type": "integer",
            "description": "Max lines to return. Omit for default, 0 for no limit."
        }),
    );
}

fn take_usize(args: &mut Map<String, Value>, key: &str) -> Result<Option<usize>, ToolError> {
    match args.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(v) => Ok(Some(v as usize)),
            None => Err(bad_integer(key)),
        },
        Some(Value::String(s)) => match s.trim().parse::<usize>() {
            Ok(v) => Ok(Some(v)),
            Err(_) => Err(bad_integer(key)),
        },
        Some(_) => Err(bad_integer(key)),
    }
}

fn bad_integer(key: &str) -> ToolError {
    ToolError::ToolCallError(crate::t!("paginate-bad-integer", key = key.to_string()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_output_is_returned_unchanged() {
        let out = paginate_text("a\nb\nc\n", None, None, 0);
        assert_eq!(out, "a\nb\nc\n");
    }

    #[test]
    fn empty_output_is_passed_through() {
        assert_eq!(paginate_text("", None, None, 10), "");
        assert_eq!(
            paginate_text("(empty file)", None, None, 10),
            "(empty file)"
        );
    }

    #[test]
    fn offset_starts_at_requested_line() {
        let out = paginate_text("a\nb\nc\nd\n", Some(2), None, 0);
        let head = crate::t!("paginate-showing", start = 2, end = 4, total = 4);
        assert!(out.starts_with(&format!("{head}\n")));
        assert!(!out.contains('a'));
        assert!(out.contains("b\n"));
        assert!(out.contains("d\n"));
    }

    #[test]
    fn limit_caps_lines_and_hints_next_offset() {
        let out = paginate_text("a\nb\nc\nd\ne\n", Some(1), Some(2), 0);
        let head = crate::t!("paginate-showing", start = 1, end = 2, total = 5);
        assert!(out.starts_with(&format!("{head}\n")));
        assert!(out.contains("a\nb\n"));
        assert!(!out.contains("\nc\n"));
        assert!(out.contains(&crate::t!("paginate-more-lines", count = 3, offset = 3)));
    }

    #[test]
    fn default_limit_applies_when_limit_omitted() {
        let out = paginate_text("a\nb\nc\nd\ne\n", None, None, 2);
        let head = crate::t!("paginate-showing", start = 1, end = 2, total = 5);
        assert!(out.starts_with(&format!("{head}\n")));
        assert!(out.contains(&crate::t!("paginate-more-lines", count = 3, offset = 3)));
    }

    #[test]
    fn explicit_zero_limit_overrides_default_and_returns_all() {
        let out = paginate_text("a\nb\nc\nd\ne\n", None, Some(0), 2);
        assert_eq!(out, "a\nb\nc\nd\ne\n");
    }

    #[test]
    fn offset_of_zero_is_clamped_to_one_at_helper_level() {
        let out = paginate_text("a\nb\n", Some(0), Some(1), 0);
        assert!(out.contains("a\n"));
        assert!(!out.contains("\nb\n"));
    }

    #[test]
    fn offset_past_end_reports_total() {
        let out = paginate_text("a\nb\n", Some(9), None, 0);
        assert_eq!(out, crate::t!("paginate-past-end", offset = 9, total = 2));
    }

    struct RawEcho;
    impl ToolDyn for RawEcho {
        fn name(&self) -> String {
            "raw_echo".to_string()
        }
        fn description(&self) -> String {
            "echo".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            })
        }
        fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
            Box::pin(async move { Ok(args) })
        }
    }

    struct FixedLines;
    impl ToolDyn for FixedLines {
        fn name(&self) -> String {
            "fixed_lines".to_string()
        }
        fn description(&self) -> String {
            "fixed".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn call<'a>(&'a self, _args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
            Box::pin(async { Ok("l1\nl2\nl3\nl4\nl5\n".to_string()) })
        }
    }

    #[tokio::test]
    async fn definition_adds_offset_and_limit_params() {
        let tool = paginate(0, vec![Box::new(RawEcho) as Box<dyn ToolDyn>])
            .pop()
            .unwrap();
        let definition = rig::tool::tool_definition(&*tool);
        let properties = definition.parameters["properties"].as_object().unwrap();
        assert!(properties.contains_key("text"));
        assert!(properties.contains_key("offset"));
        assert!(properties.contains_key("limit"));
    }

    #[tokio::test]
    async fn pagination_params_are_stripped_before_reaching_inner_tool() {
        let tool = paginate(0, vec![Box::new(RawEcho) as Box<dyn ToolDyn>])
            .pop()
            .unwrap();
        let output = tool
            .call(r#"{"text":"hi","offset":1,"limit":5}"#.to_string())
            .await
            .unwrap();
        let echoed: Map<String, Value> = serde_json::from_str(&output).unwrap();
        assert!(echoed.contains_key("text"));
        assert!(!echoed.contains_key("offset"));
        assert!(!echoed.contains_key("limit"));
    }

    #[tokio::test]
    async fn call_paginates_inner_output() {
        let tool = paginate(0, vec![Box::new(FixedLines) as Box<dyn ToolDyn>])
            .pop()
            .unwrap();
        let output = tool
            .call(r#"{"offset":2,"limit":2}"#.to_string())
            .await
            .unwrap();
        assert!(output.starts_with(&format!(
            "{}\n",
            crate::t!("paginate-showing", start = 2, end = 3, total = 5)
        )));
        assert!(output.contains("l2\n"));
        assert!(output.contains("l3\n"));
        assert!(!output.contains("l1"));
        assert!(!output.contains("l4"));
        assert!(output.contains(&crate::t!("paginate-more-lines", count = 2, offset = 4)));
    }

    #[tokio::test]
    async fn default_limit_paginates_without_explicit_args() {
        let tool = paginate(2, vec![Box::new(FixedLines) as Box<dyn ToolDyn>])
            .pop()
            .unwrap();
        let output = tool.call("{}".to_string()).await.unwrap();
        assert!(output.starts_with(&format!(
            "{}\n",
            crate::t!("paginate-showing", start = 1, end = 2, total = 5)
        )));
        assert!(output.contains(&crate::t!("paginate-more-lines", count = 3, offset = 3)));
    }

    #[tokio::test]
    async fn rejects_non_integer_offset() {
        let tool = paginate(0, vec![Box::new(RawEcho) as Box<dyn ToolDyn>])
            .pop()
            .unwrap();
        let result = tool
            .call(r#"{"text":"hi","offset":"abc"}"#.to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn paginate_returns_tool_vec_for_vec_input() {
        let tools: Vec<Box<dyn ToolDyn>> = paginate(
            0,
            vec![Box::new(RawEcho) as Box<dyn ToolDyn>, Box::new(FixedLines)],
        );
        assert_eq!(tools.len(), 2);
    }

    #[tokio::test]
    async fn offset_zero_rejected_at_tool_level() {
        let tool = paginate(0, vec![Box::new(RawEcho) as Box<dyn ToolDyn>])
            .pop()
            .unwrap();
        let result = tool.call(r#"{"text":"hi","offset":0}"#.to_string()).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(&crate::t!("paginate-offset-one-based")),
            "expected error about offset being 1-based, got: {err_msg}"
        );
    }
}
