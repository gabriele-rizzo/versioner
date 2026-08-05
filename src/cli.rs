use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author = "Gabriele Rizzo", long_about = None)]
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
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Major { message } => message.clone(),
            Self::Minor { message } => message.clone(),
            Self::Patch { message } => message.clone(),
        }
    }
}
