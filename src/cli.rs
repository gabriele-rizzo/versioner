use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author = "Gabriele Rizzo", version, long_about = None)]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum Commands {
    Major { message: String },
    Minor { message: String },
    Patch { message: String },
}

impl Commands {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Major { message } | Self::Minor { message } | Self::Patch { message } => message,
        }
    }
}
