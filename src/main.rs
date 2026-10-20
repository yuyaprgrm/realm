use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "realm", about = "Nix-native Linux sandbox runtime")]
struct Args {
    #[arg(short, long, default_value = "realm.nix", value_name = "FILE")]
    config: std::path::PathBuf,
    #[arg(value_name = "SANDBOX")]
    sandbox: String,
    #[arg(last = true)]
    command: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    Ok(realm::sandbox::run(
        &args.config,
        &args.sandbox,
        &args.command,
    )?)
}
