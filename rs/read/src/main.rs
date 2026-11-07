#[tokio::main]
async fn main() {
    let args = std::env::args_os().skip(1).collect();
    let exit = verdaxis_read::cli::dispatch_os(args).await;
    std::process::exit(exit as i32);
}
