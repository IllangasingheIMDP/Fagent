use std::env;
use std::fs;
use std::path::PathBuf;

use clap::ValueEnum;
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use inquire::validator::Validation;
use inquire::{Password, PasswordDisplayMode, Select, Text};
use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::{FagentError, Result};

const SERVICE_NAME: &str = "fagent";

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    OpenAi,
    Groq,
    Anthropic,
    Gemini,
    Ollama,
}

impl ProviderKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Groq => "Groq",
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Gemini",
            Self::Ollama => "Ollama",
        }
    }

    pub fn keychain_account(&self) -> &'static str {
        match self {
            Self::OpenAi => "openai_api_key",
            Self::Groq => "groq_api_key",
            Self::Anthropic => "anthropic_api_key",
            Self::Gemini => "gemini_api_key",
            Self::Ollama => "ollama_api_key",
        }
    }

    pub fn env_var(&self) -> Option<&'static str> {
        match self {
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::Groq => Some("GROQ_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::Ollama => None,
        }
    }

    pub fn default_model(&self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4.1-mini",
            Self::Groq => "openai/gpt-oss-120b",
            Self::Anthropic => "claude-3-7-sonnet-latest",
            Self::Gemini => "gemini-3.5-flash-lite",
            Self::Ollama => "llama3.1:8b",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileConfig {
    pub default_provider: Option<ProviderKind>,
    pub default_model: Option<String>,
    pub ollama_base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub api_key: Option<String>,
    pub ollama_base_url: String,
    pub config_path: PathBuf,
}

pub fn run_setup() -> Result<()> {
    let provider = Select::new(
        "Select a default provider:",
        vec![
            ProviderKind::OpenAi,
            ProviderKind::Groq,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Ollama,
        ],
    )
    .prompt()?;

    let model = Text::new("Default model:")
        .with_initial_value(provider.default_model())
        .prompt()?;

    let mut config = FileConfig {
        default_provider: Some(provider.clone()),
        default_model: Some(model),
        ollama_base_url: Some(default_ollama_base_url()),
    };

    if provider == ProviderKind::Ollama {
        let ollama_base_url = Text::new("Ollama base URL:")
            .with_initial_value(&default_ollama_base_url())
            .prompt()?;
        config.ollama_base_url = Some(ollama_base_url);
    } else {
        let api_key = api_key_prompt().prompt()?;
        store_api_key(&provider, &api_key)?;
    }

    write_config(&config)?;
    println!("Saved configuration to {}", config_path()?.display());
    Ok(())
}

fn api_key_prompt() -> Password<'static> {
    Password::new("Provider API key (stored in the OS keychain):")
        .with_display_mode(PasswordDisplayMode::Masked)
        .with_display_toggle_enabled()
        .with_help_message(
            "One * per character. Press Ctrl+R to show/hide and check pasted content.",
        )
        .with_formatter(&|input| "*".repeat(input.chars().count()))
        .with_validator(|input: &str| {
            Ok(if input.is_empty() {
                Validation::Invalid("Enter an API key.".into())
            } else if input.chars().any(char::is_whitespace) {
                Validation::Invalid(
                    "API keys cannot contain whitespace. Check the pasted content.".into(),
                )
            } else {
                Validation::Valid
            })
        })
        .without_confirmation()
}

fn resolve_provider_model(
    cli_provider: Option<ProviderKind>,
    cli_model: Option<String>,
    config: &FileConfig,
) -> (ProviderKind, String) {
    let provider = cli_provider
        .or_else(|| config.default_provider.clone())
        .unwrap_or(ProviderKind::OpenAi);
    let model = cli_model
        .or_else(|| {
            // A saved model belongs to its saved provider, not a CLI override.
            (config
                .default_provider
                .as_ref()
                .unwrap_or(&ProviderKind::OpenAi)
                == &provider)
                .then(|| config.default_model.clone())
                .flatten()
        })
        .unwrap_or_else(|| provider.default_model().to_string());
    (provider, model)
}

pub fn resolve_runtime(
    cli_provider: Option<ProviderKind>,
    cli_model: Option<String>,
) -> Result<ResolvedConfig> {
    let (file_config, config_path) = load_file_config()?;
    let (provider, model) = resolve_provider_model(cli_provider, cli_model, &file_config);
    let ollama_base_url = file_config
        .ollama_base_url
        .or_else(|| env::var("OLLAMA_BASE_URL").ok())
        .unwrap_or_else(default_ollama_base_url);
    let api_key = read_api_key(&provider)?;

    if provider != ProviderKind::Ollama && api_key.is_none() {
        return Err(FagentError::Config(format!(
            "missing API key for {}. Run `fagent setup` or set {}.",
            provider.label(),
            provider
                .env_var()
                .unwrap_or("the matching API key environment variable")
        )));
    }

    Ok(ResolvedConfig {
        provider,
        model,
        api_key,
        ollama_base_url,
        config_path,
    })
}

pub fn load_file_config() -> Result<(FileConfig, PathBuf)> {
    let path = config_path()?;
    let mut figment = Figment::from(Serialized::defaults(FileConfig::default()));
    if path.exists() {
        figment = figment.merge(Toml::file(&path));
    }

    let config = figment.extract()?;
    Ok((config, path))
}

pub fn config_path() -> Result<PathBuf> {
    if let Ok(appdata) = env::var("APPDATA") {
        return Ok(PathBuf::from(appdata).join("fagent").join("config.toml"));
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("fagent").join("config.toml"));
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("fagent")
            .join("config.toml"));
    }

    Err(FagentError::Config(
        "could not determine a platform config directory".into(),
    ))
}

fn write_config(config: &FileConfig) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = toml::to_string_pretty(config)
        .map_err(|error| FagentError::Config(format!("failed to serialize config: {error}")))?;
    fs::write(path, payload)?;
    Ok(())
}

fn store_api_key(provider: &ProviderKind, api_key: &str) -> Result<()> {
    let entry = Entry::new(SERVICE_NAME, provider.keychain_account())?;
    entry.set_password(api_key)?;
    let stored = entry.get_password().map_err(|error| {
        FagentError::Config(format!(
            "stored the API key for {}, but could not read it back from secure storage: {error}",
            provider.label()
        ))
    })?;
    if stored != api_key {
        return Err(FagentError::Config(format!(
            "secure storage returned a different API key for {} after writing it",
            provider.label()
        )));
    }
    Ok(())
}

fn read_api_key(provider: &ProviderKind) -> Result<Option<String>> {
    if let Some(var_name) = provider.env_var() {
        if let Ok(value) = env::var(var_name) {
            if !value.trim().is_empty() {
                return Ok(Some(value));
            }
        }
    }

    if provider == &ProviderKind::Ollama {
        return Ok(None);
    }

    let entry = Entry::new(SERVICE_NAME, provider.keychain_account())?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn default_ollama_base_url() -> String {
    "http://127.0.0.1:11434".to_string()
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groq_override_uses_its_own_default_model() {
        let config = FileConfig {
            default_provider: Some(ProviderKind::OpenAi),
            default_model: Some("saved-openai-model".into()),
            ..Default::default()
        };
        let (provider, model) = resolve_provider_model(Some(ProviderKind::Groq), None, &config);
        assert_eq!(provider, ProviderKind::Groq);
        assert_eq!(model, "openai/gpt-oss-120b");
        let (_, model) = resolve_provider_model(None, None, &config);
        assert_eq!(model, "saved-openai-model");
        let (_, model) =
            resolve_provider_model(Some(ProviderKind::Groq), Some("custom".into()), &config);
        assert_eq!(model, "custom");
    }

    #[test]
    fn groq_config_round_trips() {
        let config = FileConfig {
            default_provider: Some(ProviderKind::Groq),
            default_model: Some(ProviderKind::Groq.default_model().into()),
            ..Default::default()
        };
        let saved = toml::to_string(&config).unwrap();
        let loaded: FileConfig = toml::from_str(&saved).unwrap();
        assert_eq!(loaded.default_provider, config.default_provider);
        assert_eq!(loaded.default_model, config.default_model);
        assert_eq!(ProviderKind::Groq.env_var(), Some("GROQ_API_KEY"));
        assert_eq!(ProviderKind::Groq.keychain_account(), "groq_api_key");
    }

    #[test]
    fn api_key_prompt_masks_actual_length_and_validates_input() {
        let prompt = api_key_prompt();
        assert_eq!(prompt.display_mode, PasswordDisplayMode::Masked);
        assert_eq!(prompt.render_config.password_mask, '*');
        for input in ["", "x", "test-api-key", "é🔑"] {
            assert_eq!((prompt.formatter)(input), "*".repeat(input.chars().count()));
        }
        for input in ["", "   ", "key with spaces", "key\n", "\tkey"] {
            assert!(matches!(
                prompt.validators[0].validate(input).unwrap(),
                Validation::Invalid(_)
            ));
        }
        assert!(matches!(
            prompt.validators[0].validate("test-api-key").unwrap(),
            Validation::Valid
        ));
    }
}
