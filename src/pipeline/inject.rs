use crate::pipeline::{Inner, args_object, flatten};
use rig::tool::DynamicTool;
use serde_json::{Map, Value};

pub const CWD_PARAM: &str = "cwd";

/// Inject hidden runtime parameters (e.g. `cwd`) into every tool call.
pub fn inject(params: Map<String, Value>, tools: Vec<DynamicTool>) -> Vec<DynamicTool> {
    tools
        .into_iter()
        .map(|tool| {
            let definition = tool.definition();
            let mut parameters = definition.parameters.clone();
            hide_injected_params(&mut parameters, &params);
            let inner = Inner::new(tool);
            let params = params.clone();
            DynamicTool::new(
                definition.name,
                definition.description,
                parameters,
                move |context, args| {
                    let inner = inner.clone();
                    let params = params.clone();
                    Box::pin(async move {
                        let mut object = args_object(args)?;
                        for (key, value) in &params {
                            object.insert(key.clone(), value.clone());
                        }
                        flatten(inner.call(Value::Object(object).to_string(), context).await)
                    })
                },
            )
        })
        .collect()
}

fn hide_injected_params(parameters: &mut Value, params: &Map<String, Value>) {
    let Some(schema) = parameters.as_object_mut() else {
        return;
    };
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for key in params.keys() {
            properties.remove(key);
        }
    }
    if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|item| match item.as_str() {
            Some(key) => !params.contains_key(key),
            None => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{adapt, call};
    use rig::tool::{Tool, ToolContext};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Deserialize, JsonSchema, Serialize)]
    struct EchoArgs {
        text: Option<String>,
        cwd: String,
        memory: Value,
    }
    #[derive(Clone, Copy)]
    struct Echo;
    #[derive(Debug, thiserror::Error)]
    #[error("echo error")]
    struct EchoError;
    impl Tool for Echo {
        const NAME: &'static str = "echo";
        type Error = EchoError;
        type Args = EchoArgs;
        type Output = EchoArgs;
        fn description(&self) -> String {
            "Echo arguments".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::to_value(schemars::schema_for!(EchoArgs)).unwrap()
        }
        async fn call(
            &self,
            _context: &mut ToolContext,
            args: Self::Args,
        ) -> Result<Self::Output, Self::Error> {
            Ok(args)
        }
    }

    fn test_params() -> Map<String, Value> {
        let mut params = Map::new();
        params.insert(CWD_PARAM.into(), "/tmp/project".into());
        params.insert(
            "memory".into(),
            json!({
                "project": "togi",
                "rule": "important runtime facts are injected, not guessed"
            }),
        );
        params
    }

    #[tokio::test]
    async fn inject_adds_hidden_params_to_tool_calls() {
        let tools = inject(test_params(), vec![adapt(Echo), adapt(Echo)]);
        assert_eq!(tools.len(), 2);
        for tool in tools {
            let output = call(&tool, "null").await.unwrap();
            let output: EchoArgs = serde_json::from_str(&output).unwrap();
            assert_eq!(output.cwd, "/tmp/project");
            assert_eq!(
                output.memory["rule"],
                "important runtime facts are injected, not guessed"
            );
        }
    }

    #[tokio::test]
    async fn injected_values_override_model_arguments() {
        let tool = inject(test_params(), vec![adapt(Echo)]).pop().unwrap();
        let output = call(
            &tool,
            &json!({
                "text": "hello",
                "cwd": "/hallucinated/path",
                "memory": {"project": "wrong"}
            })
            .to_string(),
        )
        .await
        .unwrap();
        let output: EchoArgs = serde_json::from_str(&output).unwrap();
        assert_eq!(output.cwd, "/tmp/project");
        assert_eq!(output.memory["project"], "togi");
    }

    #[tokio::test]
    async fn definition_hides_all_injected_params() {
        let tool = inject(test_params(), vec![adapt(Echo)]).pop().unwrap();
        let definition = tool.definition();
        let properties = definition.parameters["properties"].as_object().unwrap();
        let required = definition.parameters["required"].as_array().unwrap();
        assert!(!properties.contains_key("cwd"));
        assert!(!properties.contains_key("memory"));
        assert!(!required.iter().any(|item| item == "cwd"));
        assert!(!required.iter().any(|item| item == "memory"));
        assert!(properties.contains_key("text"));
    }
}
