use poly_latency_bot::app;

#[tokio::main]
async fn main() {
    if let Err(e) = app::run().await {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    }
}
