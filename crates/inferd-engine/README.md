# inferd-engine

Backend adapter crate. Defines the `Backend` trait that abstracts
over different inference sources (llamafile subprocess today; Ollama
/ OpenAI / Bedrock / Anthropic / LiteLLM in v0.2).

**Status: not yet implemented** — starts in milestone M2.

## Trait sketch

```rust
#[async_trait]
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    fn ready(&self) -> bool;
    async fn generate(&self, req: GenerateRequest) -> Result<TokenStream>;
    async fn stop(&self, timeout: Duration) -> Result<()>;
}
```

See `../../docs/plan-v0.1.md` for what each adapter implementation
needs to satisfy.
