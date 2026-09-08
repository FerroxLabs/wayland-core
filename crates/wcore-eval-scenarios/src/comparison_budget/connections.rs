//! Inbound connections stay owned until shutdown joins their cancellation.
use axum::Router;
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use tokio::{net::TcpListener, sync::oneshot, task::JoinSet};

pub(super) async fn serve(
    listener: TcpListener,
    app: Router,
    mut stopped: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut stopped => break Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => break Err(error),
                };
                let service = TowerToHyperService::new(app.clone());
                connections.spawn(async move {
                    if hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service).await.is_err()
                    {
                        // No request/response data or credentials enter diagnostics.
                        tracing::debug!("comparison peer connection ended with an HTTP error");
                    }
                });
            }
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if result.is_err() {
                    tracing::debug!("comparison peer connection task did not complete normally");
                }
            }
        }
    };
    // Dropping these connection futures closes their TCP streams, including
    // clients still sending bodies or refusing to read responses. Accounting
    // workers are separately owned and must finish settlement.
    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    result
}
