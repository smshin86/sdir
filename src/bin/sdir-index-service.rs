#[cfg(windows)]
fn main() {
    if sdir::live_search::service::dispatch().is_err() {
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    std::process::exit(1);
}
