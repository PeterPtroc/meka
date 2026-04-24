//! OpenAI-compatible provider. Targets the Chat Completions API and works
//! with any compatible endpoint (vLLM, Together, Groq, local proxies, etc.)
//! via `--base-url` + `OPENAI_API_KEY`.
//!
//! No `oauth` submodule by design: OpenAI uses standard bearer-token auth
//! with no Claude-Code-style attestation / billing-header machinery, so
//! there's no separable logic to split off.

pub mod api;

pub use api::OpenAiProvider;
