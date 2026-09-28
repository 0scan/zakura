//! Dedicated HTTP listener for the explorer REST API.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, sync::Semaphore, task::JoinHandle};
use tower::BoxError;
use zakura_state::ReadState;

use crate::Indexer;

use super::{config::Config, routes};

const MAX_CONNECTIONS: usize = 128;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

type ServerTask = JoinHandle<Result<(), BoxError>>;

/// Starts the explorer REST API when a listen address is configured.
///
/// Returns a pending task and no bound address when the API is disabled.
pub async fn init<State>(
    config: Config,
    indexer: Indexer,
    read_state: State,
) -> Result<(ServerTask, Option<SocketAddr>), BoxError>
where
    State: ReadState,
{
    let Some(listen_addr) = config.listen_addr else {
        let task = tokio::spawn(std::future::pending::<Result<(), BoxError>>());
        return Ok((task, None));
    };

    tracing::info!(%listen_addr, "opening explorer REST API endpoint");
    let listener = TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, "opened explorer REST API endpoint");

    let task = tokio::spawn(run(listener, indexer, read_state));
    Ok((task, Some(local_addr)))
}

async fn run<State>(
    listener: TcpListener,
    indexer: Indexer,
    read_state: State,
) -> Result<(), BoxError>
where
    State: ReadState,
{
    let connection_permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));

    loop {
        let permit = connection_permits
            .clone()
            .acquire_owned()
            .await
            .expect("the explorer connection semaphore is never closed");
        let (stream, peer_addr) = listener.accept().await?;
        let indexer = indexer.clone();
        let read_state = read_state.clone();

        tokio::spawn(async move {
            let _permit = permit;
            let service = hyper::service::service_fn(move |request| {
                routes::handle(request, indexer.clone(), read_state.clone())
            });

            match tokio::time::timeout(
                CONNECTION_TIMEOUT,
                http1::Builder::new().serve_connection(TokioIo::new(stream), service),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::debug!(?error, %peer_addr, "explorer REST API connection closed with error");
                }
                Err(_) => {
                    tracing::debug!(%peer_addr, "explorer REST API connection timed out");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    use zakura_chain::parameters::Network;
    use zakura_state::{ReadRequest, ReadResponse};

    use crate::Indexer;

    use super::{init, Config};

    #[tokio::test]
    async fn serves_blocks_on_a_dedicated_http_listener() {
        let indexer =
            Indexer::open_ephemeral(Network::Mainnet).expect("ephemeral test indexer should open");
        let config = Config {
            listen_addr: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        };
        let read_state = tower::service_fn(|request: ReadRequest| async move {
            Err::<ReadResponse, tower::BoxError>(
                format!("unexpected test state request: {request:?}").into(),
            )
        });
        let (server, listen_addr) = init(config, indexer, read_state)
            .await
            .expect("explorer test server should start");
        let listen_addr = listen_addr.expect("configured server should bind a socket");

        let mut connection = TcpStream::connect(listen_addr)
            .await
            .expect("test client should connect");
        connection
            .write_all(
                b"GET /api/v1/blocks?limit=2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("test request should be written");
        let mut response = Vec::new();
        connection
            .read_to_end(&mut response)
            .await
            .expect("test response should be readable");
        let response = String::from_utf8(response).expect("HTTP response should be UTF-8");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""blocks":[]"#));
        assert!(response.contains(r#""limit":2"#));

        server.abort();
    }
}
