fn main() {
    if let Err(error) = weftwise::run() {
        eprintln!("weftwise: startup failed: {error}");
        std::process::exit(1);
    }
}
