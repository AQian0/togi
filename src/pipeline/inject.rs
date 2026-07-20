use crate::pipeline::tool_pipeline::ApplyLayer;
use crate::shared::util::parse_args_object;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use serde_json::{Map, Value};
pub const CWD_PARAM: &str = "cwd";

#[derive(Clone, Debug, Default)]
pub struct Injection {
    params: Map<String, Value>,
}
impl Injection {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn value(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }
    fn into_params(self) -> Map<String, Value> {
        self.params
    }
}
pub fn inject<T, Shape>(injection: impl Into<Injection>, tools: T) -> T::Output
where
    T: ApplyLayer<Shape>,
{
    let params = injection.into().into_params();
    tools.apply(move |tool| wrap(tool, params.clone()))
}
struct InjectedTool {
    inner: Box<dyn ToolDyn>,
    params: Map<String, Value>,
}
impl ToolDyn for InjectedTool {
    fn name(&self) -> String {
        self.inner.name()
    }
    fn description(&self) -> String {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        let mut parameters = self.inner.parameters();
        hide_injected_params(&mut parameters, &self.params);
        parameters
    }
    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let mut args = parse_args_object(&args)?;
            for (key, value) in &self.params {
                args.insert(key.clone(), value.clone());
            }
            let args = serde_json::to_string(&args).map_err(ToolError::JsonError)?;
            self.inner.call(args).await
        })
    }
}
fn wrap(inner: Box<dyn ToolDyn>, params: Map<String, Value>) -> Box<dyn ToolDyn> {
    Box::new(InjectedTool { inner, params })
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
    use rig::tool::Tool;
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
        async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok(args)
        }
    }
    fn test_injection() -> Injection {
        Injection::new().value(CWD_PARAM, "/tmp/project").value(
            "memory",
            json!({
                "project": "togi",
                "rule": "important runtime facts are injected, not guessed"
            }),
        )
    }
    #[tokio::test]
    async fn inject_returns_single_tool_for_single_input() {
        let tool: Box<dyn ToolDyn> = inject(test_injection(), Echo);
        let output = tool.call(r#"{"text":"hello"}"#.to_string()).await.unwrap();
        let output: EchoArgs = serde_json::from_str(&output).unwrap();
        assert_eq!(output.text.as_deref(), Some("hello"));
        assert_eq!(output.cwd, "/tmp/project");
        assert_eq!(output.memory["project"], "togi");
    }
    #[tokio::test]
    async fn inject_returns_tool_vec_for_vec_input() {
        let tools: Vec<Box<dyn ToolDyn>> = inject(
            test_injection(),
            vec![Box::new(Echo) as Box<dyn ToolDyn>, Box::new(Echo)],
        );
        assert_eq!(tools.len(), 2);
        for tool in tools {
            let output = tool.call("null".to_string()).await.unwrap();
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
        let tool = inject(test_injection(), Echo);
        let output = tool
            .call(
                json!({
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
        let tool = inject(test_injection(), Echo);
        let definition = rig::tool::tool_definition(&*tool);
        let properties = definition.parameters["properties"].as_object().unwrap();
        let required = definition.parameters["required"].as_array().unwrap();
        assert!(!properties.contains_key("cwd"));
        assert!(!properties.contains_key("memory"));
        assert!(!required.iter().any(|item| item == "cwd"));
        assert!(!required.iter().any(|item| item == "memory"));
        assert!(properties.contains_key("text"));
    }
}
