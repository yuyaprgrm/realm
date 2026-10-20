use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "sandbox", about = "Nix-native sandbox runtime")]
struct Args {
    #[arg(value_name = "FLAKE_REF")]
    flake_ref: String,
    #[arg(last = true)]
    command: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    Ok(sandbox::sandbox::run(&args.flake_ref, &args.command)?)
}
