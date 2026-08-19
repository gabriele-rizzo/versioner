use colored::Colorize;

pub(crate) fn fatal(message: &str) -> ! {
    eprintln!("{} {}", "error:".red().bold(), message);
    std::process::exit(1)
}

pub(crate) fn warn(message: &str) {
    eprintln!("{} {}", "warning:".yellow().bold(), message);
}

pub(crate) fn info(message: &str) {
    println!("{} {}", "info:".bold(), message);
}
