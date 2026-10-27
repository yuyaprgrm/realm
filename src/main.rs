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
            let names = realm::nix::list_sandboxes(&args.config).ok();
            let mut cmd = Args::command();
            // Suggest the sandbox names declared in the selected configuration.
            if let Some(names) = &names
                && let Some(run) = cmd.get_subcommands_mut().find(|c| c.get_name() == "run")
            {
                let possible: Vec<clap::builder::PossibleValue> = names
                    .clone()
                    .into_iter()
                    .map(clap::builder::PossibleValue::new)
                    .collect();
                *run = run.clone().mut_args(|arg| {
                    if arg.get_id() == "sandbox" {
                        arg.value_parser(clap::builder::PossibleValuesParser::new(possible.clone()))
                    } else {
                        arg
                    }
                });
            }
            let mut buf = Vec::new();
            generate(shell, &mut cmd, "realm", &mut buf);
            // fish completion does not emit positional possible values; add them.
            if shell == Shell::Fish
                && let Some(names) = &names
            {
                use std::io::Write as _;
                let _ = writeln!(
                    buf,
                    "complete -c realm -n \"__fish_realm_using_subcommand run\" -f -a \"{}\"",
                    names.join(" ")
                );
            }
            use std::io::Write as _;
            std::io::stdout().write_all(&buf)?;
            Ok(())
        }
    }
}
