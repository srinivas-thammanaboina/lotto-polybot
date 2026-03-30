use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Creates a cancellation token and spawns a task that cancels it on SIGINT/SIGTERM.
pub fn install(token: CancellationToken) {
    tokio::spawn(async move {
        let ctrl_c = signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut sigterm =
                signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {
                    info!("received SIGINT, initiating shutdown");
                }
                _ = sigterm.recv() => {
                    info!("received SIGTERM, initiating shutdown");
                }
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.expect("failed to listen for ctrl-c");
            info!("received SIGINT, initiating shutdown");
        }
        token.cancel();
    });
}
