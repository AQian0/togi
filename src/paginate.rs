use crate::common::parse_args_object;
use crate::tool_pipeline::ApplyLayer;
use itertools::Itertools;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::fmt::Write;
pub const OFFSET_PARAM: &str = "offset";
pub const LIMIT_PARAM: &str = "limit";

pub fn paginate<T, Shape>(default_limit: usize, tools: T) -> T::Output
where
    T: ApplyLayer<Shape>,
{
    tools.apply(move |tool| wrap(tool, default_limit))
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
            let mut args = parse_args_object(&args)?;
            let offset = take_usize(&mut args, OFFSET_PARAM)?;
            if offset == Some(0) {
                return Err(ToolError::ToolCallError(
                    "`offset` is 1-based and must be at least 1.".into(),
                ));
            }
            let limit = take_usize(&mut args, LIMIT_PARAM)?;
            let inner_args = serde_json::to_string(&args).map_err(ToolError::JsonError)?;
            let output = self.inner.call(inner_args).await?;
            Ok(paginate_text(&output, offset, limit, self.default_limit))
        })
    }
}
fn wrap(inner: Box<dyn ToolDyn>, default_limit: usize) -> Box<dyn ToolDyn> {
    Box::new(PaginatedTool {
        inner,
        default_limit,
    })
}
fn paginate_text(
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    default_limit: usize,
) -> String {
    // 先计数总行数（惰性迭代，不分配），再按需提取分页区间。
    let total = text.lines().count();
    if total == 0 {
        return text.to_string();
    }
    // offset is validated as >=1 at the tool-call boundary; keep max(1) as
    // defense-in-depth in case this helper is ever called from another path.
    let start = offset.unwrap_or(1).max(1);
    if start > total {
        return format!("(offset {start} is past end of output; output has {total} lines)");
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
    let selected: Vec<&str> = text
        .lines()
        .skip(start_idx)
        .take(end_idx - start_idx)
        .collect_vec();
    let mut out = String::with_capacity(text.len().min(16_384) + 96);
    let _ = writeln!(
        out,
        "(showing lines {}-{} of {})",
        start_idx + 1,
        end_idx,
        total
    );
    for line in &selected {
        out.push_str(line);
        out.push('\n');
    }
    if end_idx < total {
        let _ = writeln!(
            out,
            "… ({} more lines; call again with offset {})",
            total - end_idx,
            end_idx + 1
        );
    }
    out
}
/// Typed source of the injected pagination parameters. Deriving the schema with
/// schemars keeps the `offset`/`limit` definitions in one typed place instead of
/// hand-written JSON, mirroring how the concrete tools declare their arguments.
#[derive(JsonSchema)]
#[expect(dead_code)]
struct PaginationParams {
    /// 1-based line number of this tool's output to start from. Defaults to 1.
    /// Use together with `limit` to page through large output.
    #[schemars(range(min = 1))]
    offset: u64,
    /// Maximum number of output lines to return. Omit for the default page
    /// size, or pass 0 for no limit. When more lines remain, the result ends
    /// with the `offset` to use for the next page.
    limit: u64,
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
    let generated = serde_json::to_value(schemars::schema_for!(PaginationParams))
        .expect("pagination params schema serializes");
    let Some(generated) = generated.get("properties").and_then(Value::as_object) else {
        return;
    };
    for param in [OFFSET_PARAM, LIMIT_PARAM] {
        if let Some(schema) = generated.get(param) {
            properties.insert(param.to_string(), schema.clone());
        }
    }
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
    ToolError::ToolCallError(format!("`{key}` must be a non-negative integer.").into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_output_is_returned_unchanged() {
        let out = paginate_text("a\nb\nc\n", None, None, 0);
        assert_eq!(out, "a\nb\nc\n");
        assert!(!out.contains("showing lines"));
        assert!(!out.contains("more lines"));
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
        assert!(out.starts_with("(showing lines 2-4 of 4)\n"));
        assert!(!out.contains('a'));
        assert!(out.contains("b\n"));
        assert!(out.contains("d\n"));
        assert!(!out.contains("more lines"));
    }
    #[test]
    fn limit_caps_lines_and_hints_next_offset() {
        let out = paginate_text("a\nb\nc\nd\ne\n", Some(1), Some(2), 0);
        assert!(out.starts_with("(showing lines 1-2 of 5)\n"));
        assert!(out.contains("a\nb\n"));
        assert!(!out.contains("\nc\n"));
        assert!(out.contains("3 more lines; call again with offset 3"));
    }
    #[test]
    fn default_limit_applies_when_limit_omitted() {
        let out = paginate_text("a\nb\nc\nd\ne\n", None, None, 2);
        assert!(out.starts_with("(showing lines 1-2 of 5)\n"));
        assert!(out.contains("call again with offset 3"));
    }
    #[test]
    fn explicit_zero_limit_overrides_default_and_returns_all() {
        let out = paginate_text("a\nb\nc\nd\ne\n", None, Some(0), 2);
        assert_eq!(out, "a\nb\nc\nd\ne\n");
    }
    #[test]
    fn offset_of_zero_is_clamped_to_one_at_helper_level() {
        // paginate_text still clamps offset 0 → 1 as defense-in-depth.
        // The tool-call boundary rejects offset=0 before reaching here
        // (see offset_zero_rejected_at_tool_level below).
        let out = paginate_text("a\nb\n", Some(0), Some(1), 0);
        assert!(out.contains("a\n"));
        assert!(!out.contains("\nb\n"));
    }
    #[test]
    fn offset_past_end_reports_total() {
        let out = paginate_text("a\nb\n", Some(9), None, 0);
        assert!(out.contains("past end of output"));
        assert!(out.contains("2 lines"));
    }
    #[derive(JsonSchema)]
    struct RawEchoArgs {
        #[expect(dead_code)]
        text: String,
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
            serde_json::to_value(schemars::schema_for!(RawEchoArgs)).unwrap()
        }
        fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
            Box::pin(async move { Ok(args) })
        }
    }
    #[derive(JsonSchema)]
    struct FixedLinesArgs {}
    struct FixedLines;
    impl ToolDyn for FixedLines {
        fn name(&self) -> String {
            "fixed_lines".to_string()
        }
        fn description(&self) -> String {
            "fixed".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::to_value(schemars::schema_for!(FixedLinesArgs)).unwrap()
        }
        fn call<'a>(&'a self, _args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
            Box::pin(async { Ok("l1\nl2\nl3\nl4\nl5\n".to_string()) })
        }
    }
    #[tokio::test]
    async fn definition_adds_offset_and_limit_params() {
        let tool = paginate(0, RawEcho);
        let definition = rig::tool::tool_definition(&*tool);
        let properties = definition.parameters["properties"].as_object().unwrap();
        assert!(properties.contains_key("text"));
        assert!(properties.contains_key("offset"));
        assert!(properties.contains_key("limit"));
    }
    #[tokio::test]
    async fn pagination_params_are_stripped_before_reaching_inner_tool() {
        let tool = paginate(0, RawEcho);
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
        let tool = paginate(0, FixedLines);
        let output = tool
            .call(r#"{"offset":2,"limit":2}"#.to_string())
            .await
            .unwrap();
        assert!(output.starts_with("(showing lines 2-3 of 5)\n"));
        assert!(output.contains("l2\n"));
        assert!(output.contains("l3\n"));
        assert!(!output.contains("l1"));
        assert!(!output.contains("l4"));
        assert!(output.contains("call again with offset 4"));
    }
    #[tokio::test]
    async fn default_limit_paginates_without_explicit_args() {
        let tool = paginate(2, FixedLines);
        let output = tool.call("{}".to_string()).await.unwrap();
        assert!(output.starts_with("(showing lines 1-2 of 5)\n"));
        assert!(output.contains("call again with offset 3"));
    }
    #[tokio::test]
    async fn rejects_non_integer_offset() {
        let tool = paginate(0, RawEcho);
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
        let tool = paginate(0, RawEcho);
        let result = tool.call(r#"{"text":"hi","offset":0}"#.to_string()).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("1-based") || err_msg.contains("at least 1"),
            "expected error about offset being 1-based, got: {err_msg}"
        );
    }
}
