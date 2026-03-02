use anyhow::Result;
use clap::Parser;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "PrismarineLauncher")]
#[command(about = "Rust rewrite bootstrap for PrismarineLauncher")]
struct Args {
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!(dry_run = args.dry_run, "PrismarineLauncher Rust bootstrap started");
    println!("PrismarineLauncher Rust bootstrap is ready.");
    Ok(())
}
