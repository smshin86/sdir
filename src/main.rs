#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(all(any(windows, target_os = "macos"), feature = "desktop"))]
mod ui;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--ntfs-host") {
        if let Err(error) = sdir::live_search::host(&args) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args
        .iter()
        .any(|a| a == "--worker" || a == "--search-worker")
    {
        if let Err(error) = sdir::worker::worker_main(args.iter().any(|a| a == "--search-worker")) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(all(any(windows, target_os = "macos"), feature = "desktop"))]
    {
        ui::launch();
    }
    #[cfg(not(all(any(windows, target_os = "macos"), feature = "desktop")))]
    {
        eprintln!("SDIR 데스크톱: Windows 또는 macOS에서 cargo run --features desktop");
    }
}
