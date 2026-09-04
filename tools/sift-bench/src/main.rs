mod detect;
mod guide;
mod report;
mod watch;

use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Cli {
  #[command(subcommand)]
  cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
  Quick {
    #[arg(long)]
    apps: Option<String>,
    #[arg(long, default_value = "default")]
    runs: String,
  },
  Prepare { #[arg(long)] plan: String },
  Run { #[arg(long)] plan: String, #[arg(long)] out: String },
  Report { path: String },
}

fn main() -> anyhow::Result<()> {
  let cli = Cli::parse();
  match cli.cmd {
    Cmd::Quick { apps, runs } => {
      guide::quick(apps, runs)?;
    }
    Cmd::Prepare { plan } => {
      println!("prepare {plan}");
    }
    Cmd::Run { plan, out } => {
      println!("run {plan} -> {out}");
    }
    Cmd::Report { path } => {
      report::write(&path)?;
    }
  }
  Ok(())
}
