use crate::ai_transport::AiProtocol;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiKind {
    ApiKey,
    LocalEndpoint,
    Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AiFeature {
    Assistant,
    Embeddings,
    Explanations,
}

impl AiFeature {
    pub const ALL: [Self; 3] = [Self::Assistant, Self::Embeddings, Self::Explanations];
    pub fn label(self) -> &'static str {
        match self {
            Self::Assistant => "Assistant",
            Self::Embeddings => "Embeddings",
            Self::Explanations => "Explanations",
        }
    }
    pub fn supports(self, caps: &AiCapabilities) -> bool {
        match self {
            Self::Assistant => caps.chat && caps.tools,
            Self::Embeddings => caps.embeddings,
            Self::Explanations => caps.chat,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilities {
    pub chat: bool,
    pub streaming: bool,
    pub tools: bool,
    pub embeddings: bool,
    pub context_window: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiProviderConfig {
    pub id: String,
    pub name: String,
    pub preset: AiPreset,
    pub kind: AiKind,
    pub base_url: String,
    pub model: String,
    pub capabilities: AiCapabilities,
    pub soft_limit: u64,
    pub last_test: Option<String>,
}
impl AiProviderConfig {
    pub fn protocol(&self) -> AiProtocol {
        if self.kind == AiKind::Subscription {
            AiProtocol::ChatGptPlan
        } else {
            self.preset.protocol()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiBinding {
    pub provider_id: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSettings {
    pub version: u8,
    pub revision: u64,
    pub providers: Vec<AiProviderConfig>,
    pub bindings: BTreeMap<AiFeature, AiBinding>,
    pub consents: BTreeMap<AiFeature, String>,
    pub local_only: bool,
}
impl Default for AiSettings {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            providers: Vec::new(),
            bindings: BTreeMap::new(),
            consents: BTreeMap::new(),
            local_only: true,
        }
    }
}
impl AiSettings {
    pub fn consented(&self, feature: AiFeature, id: &str) -> bool {
        self.consents.get(&feature).is_some_and(|v| v == id)
    }
    pub fn provider(&self, id: &str) -> Result<&AiProviderConfig, String> {
        self.providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| "AI provider not configured".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiPreset {
    OpenAi,
    Anthropic,
    Gemini,
    Mistral,
    Custom,
    Ollama,
    LmStudio,
    LlamaCpp,
}

/// Vendor access verified in docs/specs/32-ai-providers/ACCESS.md.
pub struct AiAccessRecord {
    pub label: &'static str,
    pub base_url: &'static str,
    pub protocol: AiProtocol,
    pub kind: AiKind,
    pub subscription: bool,
}
impl AiPreset {
    pub const ALL: [Self; 8] = [
        Self::OpenAi,
        Self::Anthropic,
        Self::Gemini,
        Self::Mistral,
        Self::Custom,
        Self::Ollama,
        Self::LmStudio,
        Self::LlamaCpp,
    ];
    pub fn access(self) -> AiAccessRecord {
        use AiKind::{ApiKey, LocalEndpoint};
        let (label, base_url, protocol, kind, subscription) = match self {
            Self::OpenAi => (
                "OpenAI",
                "https://api.openai.com/v1",
                AiProtocol::OpenAi,
                ApiKey,
                true,
            ),
            Self::Anthropic => (
                "Anthropic",
                "https://api.anthropic.com/v1",
                AiProtocol::Anthropic,
                ApiKey,
                false,
            ),
            Self::Gemini => (
                "Google Gemini",
                "https://generativelanguage.googleapis.com/v1beta",
                AiProtocol::Gemini,
                ApiKey,
                false,
            ),
            Self::Mistral => (
                "Mistral",
                "https://api.mistral.ai/v1",
                AiProtocol::OpenAi,
                ApiKey,
                false,
            ),
            Self::Custom => ("OpenAI-compatible", "", AiProtocol::OpenAi, ApiKey, false),
            Self::Ollama => (
                "Ollama",
                "http://127.0.0.1:11434/v1",
                AiProtocol::OpenAi,
                LocalEndpoint,
                false,
            ),
            Self::LmStudio => (
                "LM Studio",
                "http://127.0.0.1:1234/v1",
                AiProtocol::OpenAi,
                LocalEndpoint,
                false,
            ),
            Self::LlamaCpp => (
                "llama.cpp",
                "http://127.0.0.1:8080/v1",
                AiProtocol::OpenAi,
                LocalEndpoint,
                false,
            ),
        };
        AiAccessRecord {
            label,
            base_url,
            protocol,
            kind,
            subscription,
        }
    }
    pub fn protocol(self) -> AiProtocol {
        self.access().protocol
    }
    pub fn label(self) -> &'static str {
        self.access().label
    }
    pub fn draft(self) -> AiProviderConfig {
        let access = self.access();
        AiProviderConfig {
            id: String::new(),
            name: access.label.into(),
            preset: self,
            kind: access.kind,
            base_url: access.base_url.into(),
            model: String::new(),
            capabilities: AiCapabilities {
                chat: true,
                streaming: true,
                tools: matches!(
                    self,
                    Self::OpenAi | Self::Anthropic | Self::Gemini | Self::Mistral
                ),
                embeddings: matches!(self, Self::OpenAi | Self::Mistral | Self::Gemini),
                context_window: None,
            },
            soft_limit: 0,
            last_test: None,
        }
    }
}
