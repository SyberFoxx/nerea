use std::{
    io::{self, IsTerminal, Write},
    process::ExitCode,
    sync::Arc,
};

use clap::{Parser, Subcommand};
use futures::StreamExt;
use nerea_core::{
    factory::{self, KeyState, PlatformKeys},
    persona, presets, AiProvider, Config, Conversation, SecretStore,
};

const SERVICE: &str = "nerea";

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
    let keys = PlatformKeys::new(SERVICE);

    match cli.command {
        Command::Providers => {
            let config = Config::load()?;
            for preset in presets::all() {
                let marker = if preset.id == config.provider { "*" } else { " " };
                println!(
                    "{marker} {:<12} {:<20} {:<14} {}",
                    preset.id,
                    preset.default_model,
                    key_state(&keys, preset),
                    preset.label
                );
                println!("  {:<14} allowed: {}", "", preset.models.join(", "));
            }
            Ok(())
        }

        Command::Key { action } => match action {
            KeyAction::Set { provider } => {
                let preset = factory::find(&provider)?;
                let key = read_secret()?.trim().to_owned();
                if key.is_empty() {
                    return Err("empty key: nothing stored".into());
                }
                store.set_api_key(preset.id, &key)?;
                println!("stored key for {}", preset.id);
                Ok(())
            }
            KeyAction::Delete { provider } => {
                let preset = factory::find(&provider)?;
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

            let chain = factory::build_chain(
                &Config { provider: provider_id, model, ..config },
                &keys,
                !no_fallback,
            )?;
            for warning in &chain.warnings {
                eprintln!("warning: {warning}");
            }
            let provider = chain.provider;

            let mut conversation = Conversation::new(persona::system_prompt(), CONTEXT_BUDGET_TOKENS);

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
    let request = conversation.request();

    if !stream {
        let response = provider.chat_completion(&request).await?;
        println!("{}", response.content);
        return Ok(response.content);
    }

    let mut tokens = provider.stream_chat_completion(&request).await?;
    let mut stdout = io::stdout().lock();
    let mut reply = String::new();

    while let Some(item) = tokens.next().await {
        let delta = item?.content;
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

fn key_state(keys: &PlatformKeys, preset: &nerea_core::ProviderPreset) -> &'static str {
    match keys.state(preset) {
        KeyState::Env => "env",
        KeyState::Keyring => "keyring",
        KeyState::Missing => "missing",
        KeyState::Unavailable => "keyring error",
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
