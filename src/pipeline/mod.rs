//! 工具装饰管道：对 [`rig::tool::DynamicTool`] 进行分层包装。
//!
//! - [`inject`]：向工具调用注入运行时参数（如工作目录）。
//! - [`paginate`]：为工具输出添加按行分页能力。
//! - [`confirm`]：变更类工具执行前请求用户确认。

pub(crate) mod confirm;
pub mod inject;
pub(crate) mod paginate;

use rig::tool::{
    DynamicTool, Tool, ToolContext, ToolExecutionError, ToolOutput, ToolResult, ToolSet,
};
use serde_json::{Map, Value};
use std::sync::Arc;

/// 将类型化工具适配为 [`DynamicTool`]（装饰管道的入口）。
pub fn adapt<T: Tool + 'static>(tool: T) -> DynamicTool {
    let definition = rig::tool::tool_definition(&tool);
    let inner = Inner {
        set: Arc::new(ToolSet::from_tools(vec![tool])),
        name: definition.name.clone(),
    };
    DynamicTool::new(
        definition.name,
        definition.description,
        definition.parameters,
        move |context, args| {
            let inner = inner.clone();
            Box::pin(async move { flatten(inner.call(args.to_string(), context).await) })
        },
    )
}

/// 装饰器的内层工具句柄：经单工具 [`ToolSet`] 走 rig 的规范执行路径，克隆便宜。
#[derive(Clone)]
pub(crate) struct Inner {
    set: Arc<ToolSet>,
    name: String,
}

impl Inner {
    pub(crate) fn new(tool: DynamicTool) -> Self {
        let name = tool.name().to_string();
        Self {
            set: Arc::new(ToolSet::from_dynamic_tools(vec![tool])),
            name,
        }
    }

    pub(crate) async fn call(&self, args: String, context: &mut ToolContext) -> ToolResult {
        self.set.execute(&self.name, args, context).await
    }
}

/// 把 [`ToolResult`] 展平为动态回调的返回类型：错误与拒绝（保留 refusal
/// 标记）以 `Err` 上抛。skipped 只由运行时 hook 产生，工具体内不会出现。
pub(crate) fn flatten(result: ToolResult) -> Result<ToolOutput, ToolExecutionError> {
    if let Some(error) = result.error().or_else(|| result.refusal()) {
        return Err(error.clone());
    }
    Ok(result.output().clone())
}

/// 模型传入的 JSON 参数 → 对象；null 视为空对象（沿用原 parse_args_object 语义）。
pub(crate) fn args_object(args: Value) -> Result<Map<String, Value>, ToolExecutionError> {
    match args {
        Value::Null => Ok(Map::new()),
        Value::Object(object) => Ok(object),
        other => Err(ToolExecutionError::invalid_args(format!(
            "tool arguments must be a JSON object, got {other}"
        ))),
    }
}

/// 测试共享的动态工具执行辅助（`call` 的旧 `ToolDyn` 风格）。
#[cfg(test)]
pub(crate) async fn call(tool: &DynamicTool, args: &str) -> Result<String, String> {
    let result = Inner::new(tool.clone())
        .call(args.to_string(), &mut ToolContext::new())
        .await;
    if let Some(error) = result.error().or_else(|| result.refusal()) {
        return Err(error.message().to_string());
    }
    Ok(result.output().render())
}
