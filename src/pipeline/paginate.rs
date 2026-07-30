use crate::pipeline::{Inner, args_object, flatten};
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{Map, Value};
use std::fmt::Write;

pub const OFFSET_PARAM: &str = "offset";
pub const LIMIT_PARAM: &str = "limit";

pub fn paginate(default_limit: usize, tools: Vec<DynamicTool>) -> Vec<DynamicTool> {
    tools
        .into_iter()
        .map(|tool| {
            let definition = tool.definition();
            let mut parameters = definition.parameters.clone();
            add_pagination_params(&mut parameters);
            let inner = Inner::new(tool);
            DynamicTool::new(
                definition.name,
                definition.description,
                parameters,
                move |context, args| {
                    let inner = inner.clone();
                    Box::pin(async move {
                        let mut object = args_object(args)?;
                        let offset = take_usize(&mut object, OFFSET_PARAM)?;
                        if offset == Some(0) {
                            return Err(ToolExecutionError::invalid_args(crate::t!(
                                "paginate-offset-one-based"
                            )));
                        }
                        let limit = take_usize(&mut object, LIMIT_PARAM)?;
                        let result = inner.call(Value::Object(object).to_string(), context).await;
                        // 只分页成功的纯文本输出；结构化输出原样透传。
                        if result.is_success()
                            && let Some(text) = result.output().as_text()
                        {
                            return Ok(ToolOutput::text(paginate_text(
                                text,
                                offset,
                                limit,
                                default_limit,
                            )));
                        }
                        flatten(result)
                    })
                },
            )
        })
        .collect()
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

fn take_usize(
    args: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<usize>, ToolExecutionError> {
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

fn bad_integer(key: &str) -> ToolExecutionError {
    ToolExecutionError::invalid_args(crate::t!("paginate-bad-integer", key = key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::call;

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
    impl rig::tool::Tool for RawEcho {
        const NAME: &'static str = "raw_echo";
        type Error = std::convert::Infallible;
        type Args = Value;
        type Output = String;
        fn description(&self) -> String {
            "echo".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            })
        }
        async fn call(
            &self,
            _context: &mut rig::tool::ToolContext,
            args: Self::Args,
        ) -> Result<Self::Output, Self::Error> {
            Ok(args.to_string())
        }
    }

    struct FixedLines;
    impl rig::tool::Tool for FixedLines {
        const NAME: &'static str = "fixed_lines";
        type Error = std::convert::Infallible;
        type Args = Value;
        type Output = String;
        fn description(&self) -> String {
            "fixed".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        async fn call(
            &self,
            _context: &mut rig::tool::ToolContext,
            _args: Self::Args,
        ) -> Result<Self::Output, Self::Error> {
            Ok("l1\nl2\nl3\nl4\nl5\n".to_string())
        }
    }

    #[tokio::test]
    async fn definition_adds_offset_and_limit_params() {
        let tool = paginate(0, vec![crate::pipeline::adapt(RawEcho)])
            .pop()
            .unwrap();
        let definition = tool.definition();
        let properties = definition.parameters["properties"].as_object().unwrap();
        assert!(properties.contains_key("text"));
        assert!(properties.contains_key("offset"));
        assert!(properties.contains_key("limit"));
    }

    #[tokio::test]
    async fn pagination_params_are_stripped_before_reaching_inner_tool() {
        let tool = paginate(0, vec![crate::pipeline::adapt(RawEcho)])
            .pop()
            .unwrap();
        let output = call(&tool, r#"{"text":"hi","offset":1,"limit":5}"#)
            .await
            .unwrap();
        let echoed: Map<String, Value> = serde_json::from_str(&output).unwrap();
        assert!(echoed.contains_key("text"));
        assert!(!echoed.contains_key("offset"));
        assert!(!echoed.contains_key("limit"));
    }

    #[tokio::test]
    async fn call_paginates_inner_output() {
        let tool = paginate(0, vec![crate::pipeline::adapt(FixedLines)])
            .pop()
            .unwrap();
        let output = call(&tool, r#"{"offset":2,"limit":2}"#).await.unwrap();
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
        let tool = paginate(2, vec![crate::pipeline::adapt(FixedLines)])
            .pop()
            .unwrap();
        let output = call(&tool, "{}").await.unwrap();
        assert!(output.starts_with(&format!(
            "{}\n",
            crate::t!("paginate-showing", start = 1, end = 2, total = 5)
        )));
        assert!(output.contains(&crate::t!("paginate-more-lines", count = 3, offset = 3)));
    }

    #[tokio::test]
    async fn rejects_non_integer_offset() {
        let tool = paginate(0, vec![crate::pipeline::adapt(RawEcho)])
            .pop()
            .unwrap();
        let result = call(&tool, r#"{"text":"hi","offset":"abc"}"#).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn paginate_returns_tool_vec_for_vec_input() {
        let tools = paginate(
            0,
            vec![
                crate::pipeline::adapt(RawEcho),
                crate::pipeline::adapt(FixedLines),
            ],
        );
        assert_eq!(tools.len(), 2);
    }

    #[tokio::test]
    async fn offset_zero_rejected_at_tool_level() {
        let tool = paginate(0, vec![crate::pipeline::adapt(RawEcho)])
            .pop()
            .unwrap();
        let result = call(&tool, r#"{"text":"hi","offset":0}"#).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(&crate::t!("paginate-offset-one-based")),
            "expected error about offset being 1-based, got: {err_msg}"
        );
    }
}
