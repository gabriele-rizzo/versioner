use colored::Colorize;

pub(crate) fn fatal(message: &str) -> ! {
    println!("{} {}", "error:".red().bold(), message);
    std::process::exit(1)
}

pub(crate) fn info(message: &str) {
    println!("{} {}", "info:".bold(), message);
}
