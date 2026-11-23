use std::sync::Arc;

use async_trait::async_trait;
use futures::{StreamExt, stream};

use super::{AiProvider, ChatMessage, ChatResponse, ChatStream};
use crate::error::ProviderError;

/// Tries `primary` first and fails over to `secondary` on transient errors.
///
/// Streaming only fails over while nothing has been emitted: restarting after the
/// first token would duplicate text the user already saw.
pub struct FallbackProvider {
    primary: Arc<dyn AiProvider>,
    secondary: Arc<dyn AiProvider>,
}

impl FallbackProvider {
    pub fn new(primary: Arc<dyn AiProvider>, secondary: Arc<dyn AiProvider>) -> Self {
        Self { primary, secondary }
    }

    pub fn secondary(&self) -> &Arc<dyn AiProvider> {
        &self.secondary
    }
}

#[async_trait]
impl AiProvider for FallbackProvider {
    fn id(&self) -> &str {
        self.primary.id()
    }

    fn model(&self) -> &str {
        self.primary.model()
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatResponse, ProviderError> {
        match self.primary.chat_completion(messages).await {
            Ok(response) => Ok(response),
            Err(err) if err.is_failover_worthy() => self.secondary.chat_completion(messages).await,
            Err(err) => Err(err),
        }
    }

    async fn stream_chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatStream, ProviderError> {
        let mut primary = match self.primary.stream_chat_completion(messages).await {
            Ok(stream) => stream,
            Err(err) if err.is_failover_worthy() => {
                return self.secondary.stream_chat_completion(messages).await;
            }
            Err(err) => return Err(err),
        };

        // Peek at the first item: an error before any token is still failover-able.
        match primary.next().await {
            None => Ok(primary),
            Some(Ok(first)) => {
                Ok(stream::once(async move { Ok(first) }).chain(primary).boxed())
            }
            Some(Err(err)) if err.is_failover_worthy() => {
                self.secondary.stream_chat_completion(messages).await
            }
            Some(Err(err)) => Ok(stream::once(async move { Err(err) }).boxed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Usage;

    type Reply = fn() -> Result<ChatResponse, ProviderError>;
    type StreamReply = fn() -> Result<ChatStream, ProviderError>;

    struct Mock {
        id: &'static str,
        reply: Reply,
        stream_reply: StreamReply,
    }

    #[async_trait]
    impl AiProvider for Mock {
        fn id(&self) -> &str {
            self.id
        }

        fn model(&self) -> &str {
            "mock-model"
        }

        async fn chat_completion(
            &self,
            _messages: &[ChatMessage],
        ) -> Result<ChatResponse, ProviderError> {
            (self.reply)()
        }

        async fn stream_chat_completion(
            &self,
            _messages: &[ChatMessage],
        ) -> Result<ChatStream, ProviderError> {
            (self.stream_reply)()
        }
    }

    fn answer(provider: &'static str) -> Result<ChatResponse, ProviderError> {
        Ok(ChatResponse {
            provider: provider.to_owned(),
            content: "ok".to_owned(),
            model: "mock-model".to_owned(),
            finish_reason: None,
            usage: Some(Usage::default()),
        })
    }

    fn api(status: u16) -> ProviderError {
        ProviderError::Api { status, code: None, message: "boom".into() }
    }

    fn tokens(items: Vec<Result<String, ProviderError>>) -> Result<ChatStream, ProviderError> {
        Ok(Box::pin(stream::iter(items)))
    }

    fn primary(reply: Reply, stream_reply: StreamReply) -> Arc<dyn AiProvider> {
        Arc::new(Mock { id: "primary", reply, stream_reply })
    }

    fn secondary(reply: Reply, stream_reply: StreamReply) -> Arc<dyn AiProvider> {
        Arc::new(Mock { id: "secondary", reply, stream_reply })
    }

    async fn drain(stream: ChatStream) -> Vec<Result<String, ProviderError>> {
        let mut stream = stream;
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        items
    }

    #[tokio::test]
    async fn uses_the_secondary_when_the_primary_is_down() {
        let provider = FallbackProvider::new(
            primary(|| Err(api(503)), || Err(api(503))),
            secondary(|| answer("secondary"), || tokens(vec![Ok("hola".into())])),
        );

        let response = provider
            .chat_completion(&[ChatMessage::user("hi")])
            .await
            .expect("fallback answer");
        assert_eq!(response.provider, "secondary");

        let collected = drain(provider.stream_chat_completion(&[ChatMessage::user("hi")]).await.expect("stream")).await;
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].as_ref().expect("token"), "hola");
    }

    #[tokio::test]
    async fn does_not_fail_over_on_a_bad_request() {
        let provider = FallbackProvider::new(
            primary(|| Err(api(400)), || Err(api(400))),
            secondary(|| answer("secondary"), || tokens(vec![Ok("hola".into())])),
        );

        let err = provider
            .chat_completion(&[ChatMessage::user("hi")])
            .await
            .expect_err("400 must surface");
        assert!(!err.is_failover_worthy());
    }

    #[tokio::test]
    async fn fails_over_when_the_stream_errors_before_any_token() {
        let provider = FallbackProvider::new(
            primary(|| answer("primary"), || tokens(vec![Err(api(429))])),
            secondary(|| answer("secondary"), || tokens(vec![Ok("hola".into())])),
        );

        let collected =
            drain(provider.stream_chat_completion(&[ChatMessage::user("hi")]).await.expect("stream")).await;
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].as_ref().expect("token"), "hola");
    }

    #[tokio::test]
    async fn keeps_the_partial_answer_when_the_stream_fails_midway() {
        let provider = FallbackProvider::new(
            primary(
                || answer("primary"),
                || tokens(vec![Ok("par".into()), Ok("tial".into()), Err(api(500))]),
            ),
            secondary(|| answer("secondary"), || tokens(vec![Ok("never".into())])),
        );

        let collected =
            drain(provider.stream_chat_completion(&[ChatMessage::user("hi")]).await.expect("stream")).await;
        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0].as_ref().expect("token"), "par");
        assert_eq!(collected[1].as_ref().expect("token"), "tial");
        assert!(collected[2].is_err(), "the error is surfaced, not retried");
    }

    #[tokio::test]
    async fn reports_the_primary_identity() {
        let provider = FallbackProvider::new(
            primary(|| answer("primary"), || tokens(Vec::new())),
            secondary(|| answer("secondary"), || tokens(Vec::new())),
        );
        assert_eq!(provider.id(), "primary");
        assert_eq!(provider.secondary().id(), "secondary");
    }
}
