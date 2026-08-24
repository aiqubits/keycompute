//! Anthropic 流式路径客户端断开场景端到端测试。
//!
//! 验证：客户端在 message_start 之后、message_stop 之前断开时，executor 会
//! 立即取消 primary，不得再对 fallback 上游发起无意义的调用；usage log 仍以
//! 实际完成账号（无账号完成时回退 primary）、status=error 正确落库。
//!
//! 背景：Anthropic 流式 handler 的后台结算任务持有 executor 的 receiver 直到
//! Done/Error（保证计费完成），因此客户端断开不会触发 `tx.is_closed()`。
//! handler 在 SSE 发送失败时通过 `ctx.mark_client_disconnected()` 显式传播断开，
//! executor 据此取消活动上游并中止 fallback 链。

use futures::StreamExt;
use integration_tests::mocks::provider::MockProviderFactory;
use keycompute_billing::BillingService;
use keycompute_routing::{AccountStateStore, ProviderHealthStore};
use keycompute_types::{ExecutionPlan, ExecutionTarget, KeyComputeError, Message, RequestContext};
use llm_gateway::{GatewayConfig, GatewayExecutor};
use llm_protocol_provider::{
    HttpTransport, ProviderAdapter, StreamBox, StreamEvent, UpstreamRequest,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

/// 发送 message_start（Raw）后保持挂起的 primary，用于验证断连会主动取消上游。
#[derive(Debug)]
struct MessageStartThenPendingProvider;

#[async_trait::async_trait]
impl ProviderAdapter for MessageStartThenPendingProvider {
    fn name(&self) -> &'static str {
        "anthropic-primary"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec!["claude-test"]
    }

    async fn stream_chat(
        &self,
        _transport: &dyn HttpTransport,
        _request: UpstreamRequest,
    ) -> keycompute_types::Result<StreamBox> {
        let stream = futures::stream::iter(vec![Ok(StreamEvent::raw(
            r#"{"kind":"anthropic_sse","event":"message_start","data":{"type":"message_start","message":{"id":"msg_test"}}}"#
                .to_string(),
        ))])
        .chain(futures::stream::pending());
        Ok(Box::pin(stream))
    }
}

/// 只发送 ping（不提交可继续的消息）后等待外部触发，再由 Provider 明确
/// 宣告失败的 primary。
///
/// ping 不满足 `raw_event_commits_response`，`sent_content` 保持 false：
/// 这是客户端断开标志需要独立发挥作用的场景（客户端在线时 fallback 合法）。
#[derive(Debug)]
struct PingThenDeclaredErrorProvider {
    fail: Arc<Notify>,
}

#[async_trait::async_trait]
impl ProviderAdapter for PingThenDeclaredErrorProvider {
    fn name(&self) -> &'static str {
        "anthropic-primary"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        vec!["claude-test"]
    }

    async fn stream_chat(
        &self,
        _transport: &dyn HttpTransport,
        _request: UpstreamRequest,
    ) -> keycompute_types::Result<StreamBox> {
        let fail = Arc::clone(&self.fail);
        let stream = futures::stream::iter(vec![Ok(StreamEvent::raw(
            r#"{"kind":"anthropic_sse","event":"ping","data":{"type":"ping"}}"#.to_string(),
        ))])
        .chain(futures::stream::once(async move {
            fail.notified().await;
            Ok(StreamEvent::error(
                "provider explicitly rejected the request",
            ))
        }));
        Ok(Box::pin(stream))
    }
}

/// 记录 `stream_chat` 调用次数的 fallback Provider：一旦被调用即视为无意义 fallback。
#[derive(Debug)]
struct CountingFailProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl ProviderAdapter for CountingFailProvider {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn supported_models(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn stream_chat(
        &self,
        _transport: &dyn HttpTransport,
        _request: UpstreamRequest,
    ) -> keycompute_types::Result<StreamBox> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(KeyComputeError::ProviderError("upstream down".into()))
    }
}

/// 客户端在 message_start 之后、message_stop 之前断开：
/// - 不触发无意义 fallback（已提交 message_start + 断开标志共同阻止）
/// - usage log 以实际完成账号（无完成 → primary）、status=error 落库
#[tokio::test]
async fn anthropic_stream_disconnect_aborts_fallback_and_billing_stays_on_primary() {
    let fallback_calls = Arc::new(AtomicUsize::new(0));
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        "anthropic".to_string(),
        Arc::new(MessageStartThenPendingProvider) as Arc<dyn ProviderAdapter>,
    );
    providers.insert(
        "fallback".to_string(),
        Arc::new(CountingFailProvider {
            calls: Arc::clone(&fallback_calls),
        }) as Arc<dyn ProviderAdapter>,
    );
    let executor = GatewayExecutor::new(
        GatewayConfig {
            max_retries: 0,
            ..GatewayConfig::default()
        },
        providers,
    );

    let ctx = Arc::new(RequestContext::new(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        "claude-test",
        vec![Message::user("Hello")],
        true,
        keycompute_types::PricingSnapshot::default(),
    ));
    let primary_account_id = uuid::Uuid::new_v4();
    let mut rx = executor
        .execute(
            Arc::clone(&ctx),
            ExecutionPlan {
                primary: ExecutionTarget::new_provider(
                    "anthropic",
                    primary_account_id,
                    "http://primary",
                    "mock-key",
                ),
                fallback_chain: vec![ExecutionTarget::new_provider(
                    "fallback",
                    uuid::Uuid::new_v4(),
                    "http://fallback",
                    "mock-key",
                )],
            },
            Arc::new(AccountStateStore::new()),
            Some(Arc::new(ProviderHealthStore::new())),
        )
        .await
        .expect("execute should return receiver");

    // 1. 客户端已收到 message_start（message_start 之后、message_stop 之前）
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("stream should produce the message_start event")
        .expect("channel should stay open");
    assert!(
        matches!(first, StreamEvent::Raw { data } if data.contains("message_start")),
        "client should receive the native message_start event first"
    );

    // 2. 客户端断开：handler 检测到 SSE 发送失败后显式标记断开
    ctx.mark_client_disconnected();

    // 3. executor 必须立即取消尚未 message_stop 的 primary，中止 fallback 链，
    // 并向 handler 上报明确的断连终态。
    let second = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("executor should terminate the stream")
        .expect("channel should stay open until the terminal error");
    assert!(
        matches!(second, StreamEvent::Error { message } if message.contains("client disconnected")),
        "handler should observe the disconnect error, not a fallback response"
    );
    // 终止事件之后 channel 关闭（后台任务结束）
    assert!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("stream should close")
            .is_none(),
        "stream should close after the terminal error"
    );

    // 5. fallback 未被调用
    assert_eq!(
        fallback_calls.load(Ordering::SeqCst),
        0,
        "fallback must not be attempted after the client disconnected"
    );

    // 6. usage log 归属：没有账号完成请求 → 回退 primary，status=error
    assert_eq!(
        ctx.executed_provider_account(),
        None,
        "no account completed the request, so nothing may be attributed to a fallback"
    );
    // 走 handler 同款 finalize_and_trigger_distribution 路径（含余额扣减与分销
    // 触发；BillingService::new() 无外部配置时这些步骤均为空操作），验证
    // 真实结算链路的归属字段，而非仅验证结算本身。
    let log = BillingService::new()
        .finalize_and_trigger_distribution(
            &ctx,
            "anthropic",
            primary_account_id,
            "error",
            ctx.user_id,
        )
        .await
        .expect("billing finalization should succeed");
    assert_eq!(log.status, "error");
    assert_eq!(log.account_id, primary_account_id);
    assert_eq!(log.provider_name, "anthropic");
}

/// 断开标志必须独立生效：primary 只发过 ping（未提交内容，`sent_content` 为
/// false）时客户端断开，唯一能阻止 fallback 的就是 ctx 的断开标志。
#[tokio::test]
async fn anthropic_stream_ping_disconnect_aborts_fallback_via_disconnect_flag() {
    let fallback_calls = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(Notify::new());
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        "anthropic".to_string(),
        Arc::new(PingThenDeclaredErrorProvider {
            fail: Arc::clone(&fail),
        }) as Arc<dyn ProviderAdapter>,
    );
    providers.insert(
        "fallback".to_string(),
        Arc::new(CountingFailProvider {
            calls: Arc::clone(&fallback_calls),
        }) as Arc<dyn ProviderAdapter>,
    );
    let executor = GatewayExecutor::new(
        GatewayConfig {
            max_retries: 0,
            ..GatewayConfig::default()
        },
        providers,
    );

    let ctx = Arc::new(RequestContext::new(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        "claude-test",
        vec![Message::user("Hello")],
        true,
        keycompute_types::PricingSnapshot::default(),
    ));
    let mut rx = executor
        .execute(
            Arc::clone(&ctx),
            ExecutionPlan {
                primary: ExecutionTarget::new_provider(
                    "anthropic",
                    uuid::Uuid::new_v4(),
                    "http://primary",
                    "mock-key",
                ),
                fallback_chain: vec![ExecutionTarget::new_provider(
                    "fallback",
                    uuid::Uuid::new_v4(),
                    "http://fallback",
                    "mock-key",
                )],
            },
            Arc::new(AccountStateStore::new()),
            Some(Arc::new(ProviderHealthStore::new())),
        )
        .await
        .expect("execute should return receiver");

    // 客户端已收到 ping（keepalive，不提交消息），随后断开
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("ping should arrive")
        .expect("channel open");
    assert!(matches!(first, StreamEvent::Raw { data } if data.contains("ping")));
    ctx.mark_client_disconnected();

    // 没有断开标志时 fallback 是合法的（sent_content=false）；断开标志必须阻止它
    let second = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("executor should terminate with an error")
        .expect("channel open until the terminal error");
    assert!(
        matches!(second, StreamEvent::Error { message } if message.contains("client disconnected")),
        "disconnect flag must abort fallback and surface an error"
    );
    assert_eq!(
        fallback_calls.load(Ordering::SeqCst),
        0,
        "fallback must not be attempted after the client disconnected"
    );
    assert_eq!(ctx.executed_provider_account(), None);
}

/// 对照组：客户端保持在线时，未提交内容的 primary 失败仍应正常 fallback
/// （断开传播修复不得过度收紧正常场景）。
#[tokio::test]
async fn anthropic_stream_fallback_still_works_when_client_stays_connected() {
    let fail = Arc::new(Notify::new());
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        "anthropic".to_string(),
        Arc::new(PingThenDeclaredErrorProvider {
            fail: Arc::clone(&fail),
        }) as Arc<dyn ProviderAdapter>,
    );
    providers.insert(
        "openai".to_string(),
        Arc::new(MockProviderFactory::create_openai()) as Arc<dyn ProviderAdapter>,
    );
    let executor = GatewayExecutor::new(
        GatewayConfig {
            max_retries: 0,
            ..GatewayConfig::default()
        },
        providers,
    );

    let ctx = Arc::new(RequestContext::new(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        "claude-test",
        vec![Message::user("Hello")],
        true,
        keycompute_types::PricingSnapshot::default(),
    ));
    let fallback_account_id = uuid::Uuid::new_v4();
    let mut rx = executor
        .execute(
            Arc::clone(&ctx),
            ExecutionPlan {
                primary: ExecutionTarget::new_provider(
                    "anthropic",
                    uuid::Uuid::new_v4(),
                    "http://primary",
                    "mock-key",
                ),
                fallback_chain: vec![ExecutionTarget::new_provider(
                    "openai",
                    fallback_account_id,
                    "http://fallback",
                    "mock-key",
                )],
            },
            Arc::new(AccountStateStore::new()),
            Some(Arc::new(ProviderHealthStore::new())),
        )
        .await
        .expect("execute should return receiver");

    // ping 之后触发 Provider 明确失败：客户端在线，fallback 合法。连接重置、
    // malformed SSE 或缺失 message_stop 属于结果不确定，不得作为这个对照组。
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("ping should arrive")
        .expect("channel open");
    assert!(matches!(first, StreamEvent::Raw { data } if data.contains("ping")));
    fail.notify_one();

    // fallback 内容（"Hello from OpenAI" chunks）应到达，并以 Done 结束
    let mut saw_delta = false;
    let mut saw_done = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("stream should produce events")
    {
        match event {
            StreamEvent::Delta { .. } => saw_delta = true,
            StreamEvent::Done => {
                saw_done = true;
                break;
            }
            StreamEvent::Error { message } => {
                panic!("fallback should succeed, got error: {message}")
            }
            _ => {}
        }
    }
    assert!(
        saw_delta,
        "fallback deltas should reach the connected client"
    );
    assert!(saw_done, "fallback should complete with Done");
    assert_eq!(
        ctx.executed_provider_account(),
        Some(keycompute_types::ExecutedProviderAccount {
            provider: "openai".to_string(),
            account_id: fallback_account_id,
        }),
        "billing must use the fallback account that actually completed the request"
    );
}
