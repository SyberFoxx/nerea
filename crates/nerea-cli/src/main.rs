use std::{
    env,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use futures::StreamExt;
use nerea_core::{
    AiProvider, ChatMessage, OpenAiCompatProvider, ProviderPreset, SecretStore, presets,
};

const SERVICE: &str = "nerea";

const SYSTEM_PROMPT: &str = "You are Nerea, a concise desktop companion. \
Answer in the user's language. Keep replies short unless the user asks for depth.";

#[derive(Parser)]
#[command(name = "nerea", version, about = "Headless harness for nerea-core")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List providers and whether an API key is available
    Providers,
    /// Store or remove an API key in the system keyring
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// Send a single message to a provider
    Chat {
        /// Provider id, e.g. opencode-go
        provider: String,
        /// Model override
        #[arg(long)]
        model: Option<String>,
        /// Print tokens as they arrive
        #[arg(long)]
        stream: bool,
        #[arg(required = true, value_name = "MESSAGE")]
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
            for preset in presets::all() {
                println!(
                    "{:<12} {:<22} {:<14} {}",
                    preset.id,
                    preset.default_model,
                    key_state(preset, &store),
                    preset.label
                );
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

        Command::Chat { provider, model, stream, message } => {
            let preset = require_preset(&provider)?;
            let text = message.join(" ").trim().to_owned();
            if text.is_empty() {
                return Err("message is empty".into());
            }

            let base_url =
                env::var("NEREA_BASE_URL").unwrap_or_else(|_| preset.base_url.to_owned());
            let model = model.unwrap_or_else(|| preset.default_model.to_owned());
            let api_key = resolve_key(preset, &store)?;

            let chat = OpenAiCompatProvider::new(preset.id, base_url, api_key, model)?;
            let messages = [ChatMessage::system(SYSTEM_PROMPT), ChatMessage::user(text)];

            if stream {
                let mut tokens = chat.stream_chat_completion(&messages).await?;
                let mut stdout = io::stdout().lock();
                while let Some(item) = tokens.next().await {
                    let delta = item?;
                    if delta.is_empty() {
                        continue;
                    }
                    stdout.write_all(delta.as_bytes())?;
                    stdout.flush()?;
                }
                writeln!(stdout)?;
            } else {
                let response = chat.chat_completion(&messages).await?;
                println!("{}", response.content);
            }

            Ok(())
        }
    }
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
