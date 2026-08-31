fn main() {
    match weftwise::cli::dispatch_from_environment() {
        Ok(weftwise::cli::CliDisposition::LaunchApplication) => {
            if let Err(error) = weftwise::run() {
                eprintln!("weftwise: startup failed: {error}");
                std::process::exit(1);
            }
        }
        Ok(weftwise::cli::CliDisposition::Complete) => {}
        Err(error) => {
            eprintln!("weftwise: {error}");
            std::process::exit(2);
        }
    }
}
