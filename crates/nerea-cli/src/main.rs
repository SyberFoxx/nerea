use std::{
    env,
    io::{self, IsTerminal, Write},
    process::ExitCode,
    sync::Arc,
};

use clap::{Parser, Subcommand};
use futures::StreamExt;
use nerea_core::{
    AiProvider, Config, Conversation, FallbackProvider, OpenAiCompatProvider,
    ProviderPreset, SecretStore, presets,
};

const SERVICE: &str = "nerea";

const SYSTEM_PROMPT: &str = "You are Nerea, a concise AI desktop companion living on the user's \
Linux machine. Answer in the user's language. Keep replies short unless the user asks for depth. \
Never invent facts about the user; ask instead.";

/// Prompt budget in estimated tokens. A companion needs recent context, not a
/// 128k window; keeping it small also keeps latency and cost low.
const CONTEXT_BUDGET_TOKENS: usize = 8_000;

#[derive(Parser)]
#[command(name = "nerea", version, about = "Headless harness for nerea-core")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List providers, their allowed models and key availability
    Providers,
    /// Store or remove an API key in the system keyring
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// Show or create the configuration file
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Send a message, or start an interactive session when none is given
    Chat {
        /// Provider id; defaults to the configured one
        #[arg(long)]
        provider: Option<String>,
        /// Model override; must be allowed for the provider
        #[arg(long)]
        model: Option<String>,
        /// Print tokens as they arrive
        #[arg(long)]
        stream: bool,
        /// Do not use the configured fallback provider
        #[arg(long)]
        no_fallback: bool,
        #[arg(value_name = "MESSAGE")]
        message: Vec<String>,
    },
}

#[derive(Subcommand)]
enum KeyAction {
    /// Read a key from the terminal (or stdin) and store it in the keyring
    Set { provider: String },
    /// Remove the stored key
    Delete { provider: String },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the effective configuration
    Show,
    /// Print the config file path
    Path,
    /// Write the default configuration if none exists
    Init,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let store = SecretStore::new(SERVICE);

    match cli.command {
        Command::Providers => {
            let config = Config::load()?;
            for preset in presets::all() {
                let marker = if preset.id == config.provider { "*" } else { " " };
                println!(
                    "{marker} {:<12} {:<20} {:<14} {}",
                    preset.id,
                    preset.default_model,
                    key_state(preset, &store),
                    preset.label
                );
                println!("  {:<14} allowed: {}", "", preset.models.join(", "));
            }
            Ok(())
        }

        Command::Key { action } => match action {
            KeyAction::Set { provider } => {
                let preset = require_preset(&provider)?;
                let key = read_secret()?.trim().to_owned();
                if key.is_empty() {
                    return Err("empty key: nothing stored".into());
                }
                store.set_api_key(preset.id, &key)?;
                println!("stored key for {}", preset.id);
                Ok(())
            }
            KeyAction::Delete { provider } => {
                let preset = require_preset(&provider)?;
                store.delete_api_key(preset.id)?;
                println!("removed key for {}", preset.id);
                Ok(())
            }
        },

        Command::Config { action } => {
            let path = Config::path()?;
            match action.unwrap_or(ConfigAction::Show) {
                ConfigAction::Path => {
                    println!("{}", path.display());
                    Ok(())
                }
                ConfigAction::Show => {
                    let config = Config::load()?;
                    if path.exists() {
                        println!("# {}", path.display());
                    } else {
                        println!("# defaults (file not found: {})", path.display());
                    }
                    print!("{}", config.to_toml()?);
                    Ok(())
                }
                ConfigAction::Init => {
                    if path.exists() {
                        println!("already exists: {}", path.display());
                        return Ok(());
                    }
                    let written = Config::default().save()?;
                    println!("wrote {}", written.display());
                    Ok(())
                }
            }
        }

        Command::Chat { provider, model, stream, no_fallback, message } => {
            let config = Config::load()?;
            let provider_id = provider.unwrap_or_else(|| config.provider.clone());

            let primary = build_provider(&provider_id, model.as_deref(), &store)?;
            let provider: Arc<dyn AiProvider> = match config.fallback.as_ref() {
                Some(fallback) if !no_fallback => {
                    // A fallback without a usable key is optional, not fatal.
                    match build_provider(&fallback.provider, fallback.model.as_deref(), &store) {
                        Ok(secondary) => Arc::new(FallbackProvider::new(primary, secondary)),
                        Err(err) => {
                            eprintln!(
                                "warning: fallback `{}` unavailable, continuing without it: {err}",
                                fallback.provider
                            );
                            primary
                        }
                    }
                }
                _ => primary,
            };

            let mut conversation = Conversation::new(SYSTEM_PROMPT, CONTEXT_BUDGET_TOKENS);

            if !message.is_empty() {
                conversation.push_user(message.join(" "));
                let reply = send(&provider, &conversation, stream).await?;
                conversation.push_assistant(reply);
                Ok(())
            } else if io::stdin().is_terminal() {
                repl(provider, conversation).await
            } else {
                Err("no message given: pass text or run it from a terminal".into())
            }
        }
    }
}

async fn send(
    provider: &Arc<dyn AiProvider>,
    conversation: &Conversation,
    stream: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let messages = conversation.trimmed();

    if !stream {
        let response = provider.chat_completion(&messages).await?;
        println!("{}", response.content);
        return Ok(response.content);
    }

    let mut tokens = provider.stream_chat_completion(&messages).await?;
    let mut stdout = io::stdout().lock();
    let mut reply = String::new();

    while let Some(item) = tokens.next().await {
        let delta = item?;
        if delta.is_empty() {
            continue;
        }
        reply.push_str(&delta);
        stdout.write_all(delta.as_bytes())?;
        stdout.flush()?;
    }
    writeln!(stdout)?;

    Ok(reply)
}

async fn repl(
    provider: Arc<dyn AiProvider>,
    mut conversation: Conversation,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "nerea | {} ({}) | :quit to exit, :reset to forget the conversation",
        provider.id(),
        provider.model()
    );

    loop {
        print!("you> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }

        match line.trim() {
            "" => continue,
            ":quit" | ":q" => return Ok(()),
            ":reset" => {
                conversation.clear();
                println!("history cleared");
                continue;
            }
            input => conversation.push_user(input),
        }

        print!("nerea> ");
        io::stdout().flush()?;
        let reply = send(&provider, &conversation, true).await?;
        conversation.push_assistant(reply);
    }
}

fn build_provider(
    preset_id: &str,
    model_override: Option<&str>,
    store: &SecretStore,
) -> Result<Arc<dyn AiProvider>, Box<dyn std::error::Error>> {
    let preset = require_preset(preset_id)?;
    let model = model_override.unwrap_or(preset.default_model).to_owned();

    if !preset.allows_model(&model) {
        return Err(format!(
            "model `{model}` is not allowed for `{}` (allowed: {})",
            preset.id,
            preset.models.join(", ")
        )
        .into());
    }

    let base_url = env::var("NEREA_BASE_URL").unwrap_or_else(|_| preset.base_url.to_owned());
    let api_key = resolve_key(preset, store)?;
    let provider = OpenAiCompatProvider::new(preset.id, base_url, api_key, model)?;

    Ok(Arc::new(provider))
}

fn require_preset(id: &str) -> Result<&'static ProviderPreset, Box<dyn std::error::Error>> {
    presets::find(id).ok_or_else(|| {
        let known: Vec<&str> = presets::all().iter().map(|preset| preset.id).collect();
        format!("unknown provider `{id}` (known: {})", known.join(", ")).into()
    })
}

/// Environment wins over the keyring so development does not touch the OS store.
fn resolve_key(
    preset: &ProviderPreset,
    store: &SecretStore,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(value) = env::var(preset.env_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_owned());
        }
    }

    match store.api_key(preset.id)? {
        Some(key) => Ok(key),
        None => Err(format!(
            "no API key for `{}`: run `nerea key set {}` or export {}",
            preset.id,
            preset.id,
            preset.env_var
        )
        .into()),
    }
}

fn key_state(preset: &ProviderPreset, store: &SecretStore) -> &'static str {
    let from_env = env::var(preset.env_var)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if from_env {
        return "env";
    }

    match store.api_key(preset.id) {
        Ok(Some(_)) => "keyring",
        Ok(None) => "missing",
        Err(_) => "keyring error",
    }
}

fn read_secret() -> io::Result<String> {
    if io::stdin().is_terminal() {
        rpassword::prompt_password("API key: ")
    } else {
        let mut buffer = String::new();
        io::stdin().read_line(&mut buffer)?;
        Ok(buffer)
    }
}
