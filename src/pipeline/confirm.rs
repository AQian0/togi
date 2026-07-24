use crate::agent::{AgentEvent, AgentEventSender};
use crate::tools::{ApprovalDecision, ToolEffect, ToolRegistry};
use rig::tool::{ToolCallExtensions, ToolDyn, ToolError, ToolExecutionResult};
use rig::wasm_compat::WasmBoxedFuture;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{oneshot, watch};

/// 会话级工具放行策略；配置白名单与 UI 的“始终允许”共享同一状态。
#[derive(Clone)]
pub(crate) struct ApprovalPolicy {
    allowed: watch::Sender<HashSet<String>>,
}

impl ApprovalPolicy {
    #[must_use]
    pub(crate) fn new(names: impl IntoIterator<Item = String>) -> Self {
        let (allowed, _) = watch::channel(names.into_iter().collect());
        Self { allowed }
    }

    #[must_use]
    pub(crate) fn is_allowed(&self, name: &str) -> bool {
        self.allowed.borrow().contains(name)
    }

    pub(crate) fn allow(&self, name: &str) {
        self.allowed.send_modify(|names| {
            names.insert(name.to_string());
        });
    }

    fn subscribe(&self) -> watch::Receiver<HashSet<String>> {
        self.allowed.subscribe()
    }
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

/// 为所有工具加上统一的变更确认；只读与已放行工具直接透传。
pub(crate) fn confirm(
    tools: Vec<Box<dyn ToolDyn>>,
    registry: ToolRegistry,
    policy: ApprovalPolicy,
) -> Vec<Box<dyn ToolDyn>> {
    let registry = Arc::new(registry);
    tools
        .into_iter()
        .map(|inner| {
            Box::new(ConfirmedTool {
                name: inner.name(),
                inner,
                registry: Arc::clone(&registry),
                policy: policy.clone(),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

struct ConfirmedTool {
    name: String,
    inner: Box<dyn ToolDyn>,
    registry: Arc<ToolRegistry>,
    policy: ApprovalPolicy,
}

impl ConfirmedTool {
    async fn approval(&self, args: &str, extensions: &ToolCallExtensions) -> Result<(), String> {
        let arguments = serde_json::from_str(args)
            .unwrap_or_else(|_| serde_json::Value::String(args.to_string()));
        if self.registry.classify(&self.name, &arguments) != ToolEffect::Mutating
            || self.policy.is_allowed(&self.name)
        {
            return Ok(());
        }

        let Some(event_tx) = extensions.get::<AgentEventSender>() else {
            return Err(crate::t!("approval-unavailable", tool = self.name.as_str()));
        };
        let Some(mut cancel_rx) = extensions.get::<watch::Receiver<bool>>().cloned() else {
            return Err(crate::t!("approval-unavailable", tool = self.name.as_str()));
        };
        if *cancel_rx.borrow() {
            return Err(crate::t!("approval-cancelled", tool = self.name.as_str()));
        }

        let mut allow_rx = self.policy.subscribe();
        if allow_rx.borrow().contains(&self.name) {
            return Ok(());
        }
        let (response, decision_rx) = oneshot::channel();
        if event_tx
            .send(AgentEvent::ApprovalRequest {
                name: self.name.clone(),
                arguments,
                response,
            })
            .is_err()
        {
            return Err(crate::t!("approval-unavailable", tool = self.name.as_str()));
        }

        tokio::select! {
            biased;
            _ = cancel_rx.changed() => Err(crate::t!(
                "approval-cancelled",
                tool = self.name.as_str()
            )),
            decision = decision_rx => match decision {
                Ok(ApprovalDecision::AllowOnce) => Ok(()),
                Ok(ApprovalDecision::AlwaysAllow) => {
                    self.policy.allow(&self.name);
                    Ok(())
                }
                Ok(ApprovalDecision::Deny) => Err(crate::t!(
                    "approval-denied",
                    tool = self.name.as_str()
                )),
                Err(_) => Err(crate::t!(
                    "approval-cancelled",
                    tool = self.name.as_str()
                )),
            },
            allowed = allow_rx.wait_for(|names| names.contains(&self.name)) => allowed
                .map(|_| ())
                .map_err(|_| crate::t!(
                    "approval-unavailable",
                    tool = self.name.as_str()
                )),
        }
    }
}

impl ToolDyn for ConfirmedTool {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let extensions = ToolCallExtensions::new();
            self.call_with_extensions(args, &extensions).await
        })
    }

    fn call_with_extensions<'a>(
        &'a self,
        args: String,
        extensions: &'a ToolCallExtensions,
    ) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            match self.approval(&args, extensions).await {
                Ok(()) => self.inner.call_with_extensions(args, extensions).await,
                Err(message) => Err(ToolError::ToolCallError(Box::new(std::io::Error::other(
                    message,
                )))),
            }
        })
    }

    fn call_structured<'a>(
        &'a self,
        args: String,
        extensions: &'a ToolCallExtensions,
    ) -> WasmBoxedFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            match self.approval(&args, extensions).await {
                Ok(()) => self.inner.call_structured(args, extensions).await,
                Err(message) => ToolExecutionResult::denied(message),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentEvent, AgentEventSender};
    use crate::tools::{ApprovalDecision, ClassifyEffect};
    use rig::tool::{ToolCallExtensions, ToolError};
    use rig::wasm_compat::WasmBoxedFuture;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mutating;

    impl ClassifyEffect for Mutating {
        fn name() -> &'static str {
            "mutating"
        }
    }

    struct CountingTool {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl ToolDyn for CountingTool {
        fn name(&self) -> String {
            self.name.to_string()
        }

        fn description(&self) -> String {
            "test".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn call<'a>(&'a self, _args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok("ran".to_string())
            })
        }
    }

    fn guarded_tool(calls: Arc<AtomicUsize>) -> Box<dyn ToolDyn> {
        guarded_tool_with_policy(calls, ApprovalPolicy::default())
    }

    fn guarded_tool_with_policy(
        calls: Arc<AtomicUsize>,
        policy: ApprovalPolicy,
    ) -> Box<dyn ToolDyn> {
        let mut registry = ToolRegistry::new();
        registry.register::<Mutating>();
        confirm(
            vec![Box::new(CountingTool {
                name: Mutating::name(),
                calls,
            })],
            registry,
            policy,
        )
        .pop()
        .unwrap()
    }

    fn extensions() -> (
        ToolCallExtensions,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        watch::Sender<bool>,
    ) {
        let mut extensions = ToolCallExtensions::new();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        extensions.insert(tx as AgentEventSender);
        let (cancel_tx, _) = watch::channel(false);
        extensions.insert(cancel_tx.subscribe());
        (extensions, rx, cancel_tx)
    }

    #[tokio::test]
    async fn mutating_tool_waits_for_approval_before_running() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (extensions, mut events, _cancel_tx) = extensions();
        let mut call = Box::pin(tool.call_structured("{}".to_string(), &extensions));

        let response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut call => panic!("tool ran before approval: {result:?}"),
        };
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        response.send(ApprovalDecision::AllowOnce).unwrap();
        let result = call.await;
        assert!(result.outcome().is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn denial_prevents_mutating_tool_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (extensions, mut events, _cancel_tx) = extensions();
        let mut call = Box::pin(tool.call_structured("{}".to_string(), &extensions));
        let response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut call => panic!("tool ran before approval: {result:?}"),
        };

        response.send(ApprovalDecision::Deny).unwrap();
        let result = call.await;
        assert!(result.outcome().is_denied());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn always_allow_applies_to_later_calls_in_the_session() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (extensions, mut events, _cancel_tx) = extensions();
        let mut first = Box::pin(tool.call_structured("{}".to_string(), &extensions));
        let response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut first => panic!("tool ran before approval: {result:?}"),
        };
        response.send(ApprovalDecision::AlwaysAllow).unwrap();
        assert!(first.await.outcome().is_success());

        let second = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tool.call_structured("{}".to_string(), &extensions),
        )
        .await
        .expect("allowlisted call should not wait for UI");
        assert!(second.outcome().is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancellation_prevents_mutating_tool_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (extensions, mut events, cancel_tx) = extensions();
        let mut call = Box::pin(tool.call_structured("{}".to_string(), &extensions));
        let _response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut call => panic!("tool ran before approval: {result:?}"),
        };

        cancel_tx.send_replace(true);
        let result = call.await;
        assert!(result.outcome().is_denied());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_ui_channel_fails_closed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let result = tool
            .call_structured("{}".to_string(), &ToolCallExtensions::new())
            .await;
        assert!(result.outcome().is_denied());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn configured_allowlist_bypasses_confirmation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let policy = ApprovalPolicy::new([Mutating::name().to_string()]);
        let tool = guarded_tool_with_policy(Arc::clone(&calls), policy);
        let result = tool
            .call_structured("{}".to_string(), &ToolCallExtensions::new())
            .await;
        assert!(result.outcome().is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
