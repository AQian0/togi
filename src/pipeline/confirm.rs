use crate::agent::{AgentEvent, AgentEventSender};
use crate::pipeline::{Inner, flatten};
use crate::tools::{ApprovalDecision, ToolEffect, ToolRegistry};
use rig::tool::{DynamicTool, ToolContext, ToolExecutionError};
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
    tools: Vec<DynamicTool>,
    registry: ToolRegistry,
    policy: ApprovalPolicy,
) -> Vec<DynamicTool> {
    let registry = Arc::new(registry);
    tools
        .into_iter()
        .map(|tool| {
            let definition = tool.definition();
            let name = definition.name.clone();
            let inner = Inner::new(tool);
            let registry = Arc::clone(&registry);
            let policy = policy.clone();
            DynamicTool::new(
                definition.name,
                definition.description,
                definition.parameters,
                move |context, args| {
                    let inner = inner.clone();
                    let registry = Arc::clone(&registry);
                    let policy = policy.clone();
                    let name = name.clone();
                    Box::pin(async move {
                        match approval(&name, &args, &registry, &policy, context).await {
                            Ok(()) => flatten(inner.call(args.to_string(), context).await),
                            Err(message) => Err(ToolExecutionError::refused(message)),
                        }
                    })
                },
            )
        })
        .collect()
}

async fn approval(
    name: &str,
    args: &serde_json::Value,
    registry: &ToolRegistry,
    policy: &ApprovalPolicy,
    context: &ToolContext,
) -> Result<(), String> {
    if registry.classify(name, args) != ToolEffect::Mutating || policy.is_allowed(name) {
        return Ok(());
    }

    let Some(event_tx) = context.get::<AgentEventSender>() else {
        return Err(crate::t!("approval-unavailable", tool = name));
    };
    let Some(mut cancel_rx) = context.get::<watch::Receiver<bool>>().cloned() else {
        return Err(crate::t!("approval-unavailable", tool = name));
    };
    if *cancel_rx.borrow() {
        return Err(crate::t!("approval-cancelled", tool = name));
    }

    let mut allow_rx = policy.subscribe();
    if allow_rx.borrow().contains(name) {
        return Ok(());
    }
    let (response, decision_rx) = oneshot::channel();
    if event_tx
        .send(AgentEvent::ApprovalRequest {
            name: name.to_string(),
            arguments: args.clone(),
            response,
        })
        .is_err()
    {
        return Err(crate::t!("approval-unavailable", tool = name));
    }

    tokio::select! {
        biased;
        _ = cancel_rx.changed() => Err(crate::t!(
            "approval-cancelled",
            tool = name
        )),
        decision = decision_rx => match decision {
            Ok(ApprovalDecision::AllowOnce) => Ok(()),
            Ok(ApprovalDecision::AlwaysAllow) => {
                policy.allow(name);
                Ok(())
            }
            Ok(ApprovalDecision::Deny) => Err(crate::t!(
                "approval-denied",
                tool = name
            )),
            Err(_) => Err(crate::t!(
                "approval-cancelled",
                tool = name
            )),
        },
        allowed = allow_rx.wait_for(|names| names.contains(name)) => allowed
            .map(|_| ())
            .map_err(|_| crate::t!(
                "approval-unavailable",
                tool = name
            )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentEvent, AgentEventSender};
    use crate::pipeline::adapt;
    use crate::tools::{ApprovalDecision, ClassifyEffect};
    use rig::tool::{Tool, ToolResult, ToolSet};
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mutating;

    impl ClassifyEffect for Mutating {
        fn name() -> &'static str {
            "mutating"
        }
    }

    struct CountingTool {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for CountingTool {
        const NAME: &'static str = "mutating";
        type Error = Infallible;
        type Args = serde_json::Value;
        type Output = String;

        fn description(&self) -> String {
            "test".to_string()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn call(
            &self,
            _context: &mut ToolContext,
            _args: Self::Args,
        ) -> Result<Self::Output, Self::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("ran".to_string())
        }
    }

    async fn execute(tool: &DynamicTool, context: &mut ToolContext) -> ToolResult {
        let set = ToolSet::from_dynamic_tools(vec![tool.clone()]);
        set.execute(tool.name(), "{}", context).await
    }

    fn guarded_tool(calls: Arc<AtomicUsize>) -> DynamicTool {
        guarded_tool_with_policy(calls, ApprovalPolicy::default())
    }

    fn guarded_tool_with_policy(calls: Arc<AtomicUsize>, policy: ApprovalPolicy) -> DynamicTool {
        let mut registry = ToolRegistry::new();
        registry.register::<Mutating>();
        confirm(vec![adapt(CountingTool { calls })], registry, policy)
            .pop()
            .unwrap()
    }

    fn context() -> (
        ToolContext,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        watch::Sender<bool>,
    ) {
        let mut context = ToolContext::new();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        context.insert(tx as AgentEventSender);
        let (cancel_tx, _) = watch::channel(false);
        context.insert(cancel_tx.subscribe());
        (context, rx, cancel_tx)
    }

    #[tokio::test]
    async fn mutating_tool_waits_for_approval_before_running() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (mut context, mut events, _cancel_tx) = context();
        let mut call = Box::pin(execute(&tool, &mut context));

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
        assert!(result.is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn denial_prevents_mutating_tool_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (mut context, mut events, _cancel_tx) = context();
        let mut call = Box::pin(execute(&tool, &mut context));
        let response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut call => panic!("tool ran before approval: {result:?}"),
        };

        response.send(ApprovalDecision::Deny).unwrap();
        let result = call.await;
        assert!(result.is_refused());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn always_allow_applies_to_later_calls_in_the_session() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (mut context, mut events, _cancel_tx) = context();
        let mut first = Box::pin(execute(&tool, &mut context));
        let response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut first => panic!("tool ran before approval: {result:?}"),
        };
        response.send(ApprovalDecision::AlwaysAllow).unwrap();
        assert!(first.await.is_success());

        let second = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            execute(&tool, &mut context),
        )
        .await
        .expect("allowlisted call should not wait for UI");
        assert!(second.is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancellation_prevents_mutating_tool_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let (mut context, mut events, cancel_tx) = context();
        let mut call = Box::pin(execute(&tool, &mut context));
        let _response = tokio::select! {
            event = events.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut call => panic!("tool ran before approval: {result:?}"),
        };

        cancel_tx.send_replace(true);
        let result = call.await;
        assert!(result.is_refused());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_ui_channel_fails_closed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = guarded_tool(Arc::clone(&calls));
        let result = execute(&tool, &mut ToolContext::new()).await;
        assert!(result.is_refused());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn configured_allowlist_bypasses_confirmation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let policy = ApprovalPolicy::new([Mutating::name().to_string()]);
        let tool = guarded_tool_with_policy(Arc::clone(&calls), policy);
        let result = execute(&tool, &mut ToolContext::new()).await;
        assert!(result.is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
