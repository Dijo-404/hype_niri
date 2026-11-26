fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Err(error) = gtk::glib::MainContext::default().block_on(async {
        if args.first().is_some_and(|arg| arg.starts_with("manage-")) {
            hype_desktop::stealth_client::root(&args).await
        } else {
            hype_desktop::stealth::run(&args).await
        }
    }) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
