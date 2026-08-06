use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "slash", version, about = "Slash command-line tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate `.slash/` command definitions (spec §2.3).
    Validate {
        /// Path to the `.slash/` directory.
        #[arg(default_value = ".slash")]
        path: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate { path } => run_validate(&path),
    }
}

fn run_validate(dir: &Path) -> ExitCode {
    let entries = match collect_yaml_files(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("failed to read {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    };

    let total_bytes: u64 = entries.iter().map(|(_, size)| *size).sum();
    let mut errors: Vec<String> = Vec::new();

    if let Err(e) = slash_config::check_directory_limits(entries.len(), total_bytes) {
        errors.push(e.to_string());
    }

    let mut commands = Vec::new();
    if errors.is_empty() {
        for (path, _) in &entries {
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unknown>")
                .to_string();

            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    errors.push(format!("{filename}: failed to read file: {e}"));
                    continue;
                }
            };

            match slash_config::load_command_file(&filename, &bytes) {
                Ok(command) => commands.push((filename, command.command)),
                Err(errs) => errors.extend(errs.iter().map(ToString::to_string)),
            }
        }

        errors.extend(
            slash_config::find_duplicate_commands(&commands)
                .iter()
                .map(ToString::to_string),
        );
    }

    if errors.is_empty() {
        println!("{} command(s) valid in {}", commands.len(), dir.display());
        ExitCode::SUCCESS
    } else {
        for e in &errors {
            eprintln!("{e}");
        }
        eprintln!("{} error(s) in {}", errors.len(), dir.display());
        ExitCode::FAILURE
    }
}

fn collect_yaml_files(dir: &Path) -> std::io::Result<Vec<(PathBuf, u64)>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let is_yaml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"));
        if !is_yaml {
            continue;
        }

        let size = entry.metadata()?.len();
        entries.push((path, size));
    }
    entries.sort();
    Ok(entries)
}
