use std::process;

use clap::{Parser, Subcommand};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use whisrs::history::HistoryEntry;
use whisrs::{
    encode_message, read_message, service::ServiceManager, socket_path, Command, Response,
    RestartOutcome, State,
};

const ASCII_BANNER: &str = concat!(
    "\n",
    "         __    _\n",
    "  _    _| |__ |_|___ _ __ ___\n",
    " \\ \\//\\ / '_ \\| / __| '__/ __|\n",
    "  \\  /\\ \\ | | | \\__ \\ |  \\__ \\\n",
    "   \\/  \\/|_| |_|_|___/_|  |___/\n",
    "\n",
    "  speak. type. done.\n",
    "\n",
    env!("CARGO_PKG_VERSION"),
);

// ANSI color codes.
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

#[derive(Parser)]
#[command(
    name = "whisrs",
    about = "Linux-first voice-to-text dictation tool",
    long_version = ASCII_BANNER,
)]
struct Cli {
    #[command(subcommand)]
    command: SubCmd,
}

#[derive(Subcommand)]
enum SubCmd {
    /// Interactive onboarding — pick a backend, set API key, test microphone
    Setup,
    /// Edit any part of ~/.config/whisrs/config.toml; restarts the daemon on save
    Config,
    /// Toggle recording on/off (start dictation or stop and transcribe)
    Toggle,
    /// Cancel the current recording and discard audio
    Cancel,
    /// Query the daemon state (idle, recording, transcribing)
    Status,
    /// Show recent transcription history
    Log {
        /// Number of entries to show (default: 20)
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// Clear all history
        #[arg(long)]
        clear: bool,
    },
    /// Command mode: select text, speak an instruction, LLM rewrites it in place
    Command,
    /// Read the selected text aloud via TTS (press again to stop playback)
    #[command(alias = "read")]
    Speak,
    /// Restart the whisrs daemon (uses the systemd or OpenRC user service when present)
    Restart,
}

/// Check if stdout is a TTY for color support.
fn is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Format a state for display with optional color.
fn format_state(state: State, use_color: bool) -> String {
    if !use_color {
        return format!("{state}");
    }

    match state {
        State::Idle => format!("{BOLD}idle{RESET}"),
        State::Recording => format!("{BOLD}{GREEN}recording{RESET}"),
        State::Transcribing => format!("{BOLD}{YELLOW}transcribing{RESET}"),
        State::Synthesizing => format!("{BOLD}{CYAN}synthesizing{RESET}"),
        State::Speaking => format!("{BOLD}{GREEN}speaking{RESET}"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        SubCmd::Setup => {
            if let Err(e) = whisrs::config::setup::run_setup() {
                if is_tty() {
                    eprintln!("{RED}setup failed:{RESET} {e:#}");
                } else {
                    eprintln!("setup failed: {e:#}");
                }
                process::exit(1);
            }
        }
        SubCmd::Config => {
            if let Err(e) = whisrs::config::edit::run_config_menu() {
                if is_tty() {
                    eprintln!("{RED}config failed:{RESET} {e:#}");
                } else {
                    eprintln!("config failed: {e:#}");
                }
                process::exit(1);
            }
        }
        SubCmd::Toggle => {
            send_command(Command::Toggle).await?;
        }
        SubCmd::Cancel => {
            send_command(Command::Cancel).await?;
        }
        SubCmd::Status => {
            send_command(Command::Status).await?;
        }
        SubCmd::Log { limit, clear } => {
            if clear {
                send_command(Command::ClearHistory).await?;
            } else {
                send_command(Command::Log { limit }).await?;
            }
        }
        SubCmd::Command => {
            send_command(Command::CommandMode).await?;
        }
        SubCmd::Speak => {
            send_command(Command::Speak).await?;
        }
        SubCmd::Restart => {
            cmd_restart()?;
        }
    }

    Ok(())
}

/// Restart the whisrs daemon.
///
/// Delegates to whichever init system is managing the daemon (systemd or
/// OpenRC), and prints guidance when neither has a whisrs service installed.
/// We don't try to `pkill whisrsd` ourselves because that races with respawn
/// and silently breaks for users who launched the daemon under tmux/foot/etc.
fn cmd_restart() -> anyhow::Result<()> {
    let use_color = is_tty();
    let manager = ServiceManager::detect();

    let banner = format!("Restarting whisrs daemon ({})…", manager.name());
    if use_color {
        println!("{BOLD}{banner}{RESET}");
    } else {
        println!("{banner}");
    }

    match manager.restart() {
        RestartOutcome::Restarted => {
            if use_color {
                println!("{GREEN}Daemon restarted.{RESET}");
            } else {
                println!("Daemon restarted.");
            }
            Ok(())
        }
        RestartOutcome::Failed => {
            let hint = manager.restart_hint().unwrap_or("restart");
            if use_color {
                eprintln!("{RED}{hint} failed.{RESET}");
            } else {
                eprintln!("{hint} failed.");
            }
            process::exit(1);
        }
        RestartOutcome::NoService => {
            let detail = match manager.enable_hint() {
                Some(enable) => format!(
                    "No whisrs service installed for {}.\n\
                     \n\
                     Install it (run `whisrs setup` and accept the service step), then:\n\
                     \n\
                     \x20 {enable}\n\
                     \n\
                     Or restart the daemon manually:\n\
                     \n\
                     \x20 pkill whisrsd; sleep 0.2; whisrsd &",
                    manager.name()
                ),
                None => "No service manager detected (neither systemd nor OpenRC).\n\
                     \n\
                     Restart the daemon manually:\n\
                     \n\
                     \x20 pkill whisrsd; sleep 0.2; whisrsd &"
                    .to_string(),
            };
            if use_color {
                eprintln!("{YELLOW}{detail}{RESET}");
            } else {
                eprintln!("{detail}");
            }
            process::exit(1);
        }
    }
}

/// Connect to the daemon and send a command, printing the response.
async fn send_command(cmd: Command) -> anyhow::Result<()> {
    let path = socket_path();
    let use_color = is_tty();

    let stream = match UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(_) => {
            // Name the command that actually exists on this machine — telling
            // an OpenRC user to run systemctl is a dead end.
            let service_hint = match ServiceManager::detect().enable_hint() {
                Some(enable) => format!(
                    "\n\
                     \n\
                     Or enable the service:\n\
                     \n\
                     \x20 {enable}"
                ),
                None => String::new(),
            };
            let msg = format!(
                "whisrsd is not running. Start it with:\n\
                 \n\
                 \x20 whisrsd &{service_hint}"
            );
            if use_color {
                eprintln!("{RED}{msg}{RESET}");
            } else {
                eprintln!("{msg}");
            }
            process::exit(1);
        }
    };

    let (mut reader, mut writer) = stream.into_split();

    // Send command.
    let encoded = encode_message(&cmd)?;
    writer.write_all(&encoded).await?;
    writer.shutdown().await?;

    // Read response.
    let response: Response = read_message(&mut reader).await?;

    match response {
        Response::Ok { state } => {
            println!("{}", format_state(state, use_color));
        }
        Response::History { entries } => {
            if entries.is_empty() {
                println!("No transcription history.");
            } else {
                print_history(&entries, use_color);
            }
        }
        Response::Error { message } => {
            if use_color {
                eprintln!("{RED}error:{RESET} {message}");
            } else {
                eprintln!("error: {message}");
            }
            process::exit(1);
        }
    }

    Ok(())
}

/// Display transcription history entries.
fn print_history(entries: &[HistoryEntry], use_color: bool) {
    let dim = if use_color { "\x1b[2m" } else { "" };

    for entry in entries {
        let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
        let duration = format!("{:.1}s", entry.duration_secs);

        if use_color {
            println!(
                "{dim}{ts}{RESET}  {dim}[{backend} | {lang} | {dur}]{RESET}",
                backend = entry.backend,
                lang = entry.language,
                dur = duration,
            );
        } else {
            println!(
                "{ts}  [{backend} | {lang} | {dur}]",
                backend = entry.backend,
                lang = entry.language,
                dur = duration,
            );
        }
        println!("  {}", entry.text);
        println!();
    }
}
