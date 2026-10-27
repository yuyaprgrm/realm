use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};

#[derive(Parser, Debug)]
#[command(name = "realm", about = "Nix-native Linux sandbox runtime")]
struct Args {
    #[arg(
        short,
        long,
        default_value = "realm.nix",
        value_name = "FILE",
        global = true
    )]
    config: std::path::PathBuf,

    #[command(subcommand)]
    subcommand: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a sandbox
    #[command(alias = "up")]
    Run {
        #[arg(value_name = "SANDBOX")]
        sandbox: String,

        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Generate a shell completion script for the given shell
    #[command(alias = "completions")]
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.subcommand {
        Commands::Run { sandbox, command } => {
            let code = realm::sandbox::run(&args.config, &sandbox, &command)?;
            std::process::exit(code);
        }
        Commands::Completion { shell } => {
            let mut cmd = Args::command();
            generate(shell, &mut cmd, "realm", &mut std::io::stdout());
            Ok(())
        }
    }
}
