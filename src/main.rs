use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;

use planista::logger::Log;
use planista::{load_config, Logger, Server, Store, Wiper};

#[tokio::main]
async fn main() {
    let logger = Arc::new(Logger::stdout());
    let log: Arc<dyn Log> = logger.clone();

    let config = match load_config() {
        Ok(cfg) => cfg,
        Err(err) => logger.fatalf(format_args!("configuration: {err}")),
    };

    let store = match Store::open(&config.db_path, config.max_plans) {
        Ok(store) => Arc::new(store),
        Err(err) => logger.fatalf(format_args!("database: {err}")),
    };

    let wiper = match Wiper::new(config.base_url.clone(), config.wipe_interval, log.clone()) {
        Ok(wiper) => Arc::new(wiper),
        Err(err) => logger.fatalf(format_args!("wipe URL: {err}")),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    {
        let wiper = Arc::clone(&wiper);
        tokio::spawn(async move {
            wiper.run(shutdown_rx).await;
        });
    }

    let server = Arc::new(Server::new(
        config.clone(),
        Arc::clone(&store),
        Arc::clone(&wiper),
        log.clone(),
    ));

    let listen_addr = normalize_listen_addr(&config.listen_addr);
    let addr: SocketAddr = match listen_addr.parse() {
        Ok(addr) => addr,
        Err(err) => logger.fatalf(format_args!("listen address: {err}")),
    };

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(err) => logger.fatalf(format_args!("serve: {err}")),
    };
    logger.printf(format_args!("listening on {}", config.listen_addr));

    let graceful = GracefulShutdown::new();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("install SIGINT handler");

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                logger.print("shutting down");
                let _ = shutdown_tx.send(true);
                break;
            }
            _ = sigint.recv() => {
                logger.print("shutting down");
                let _ = shutdown_tx.send(true);
                break;
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(v) => v,
                    Err(err) => {
                        logger.printf(format_args!("accept: {err}"));
                        continue;
                    }
                };
                let io = TokioIo::new(stream);
                let server = Arc::clone(&server);
                let svc = service_fn(move |req: Request<Incoming>| {
                    let server = Arc::clone(&server);
                    async move {
                        let resp = tokio::time::timeout(
                            Duration::from_secs(30),
                            handle_with_read_timeout(server, req),
                        )
                        .await
                        .unwrap_or_else(|_| timeout_response());
                        Ok::<_, Infallible>(resp)
                    }
                });

                let mut builder = http1::Builder::new();
                builder.timer(TokioTimer::new());
                builder.header_read_timeout(Duration::from_secs(5));
                let conn = builder.serve_connection(io, svc);
                let fut = graceful.watch(conn);
                tokio::spawn(async move {
                    let _ = fut.await;
                });
            }
        }
    }

    tokio::select! {
        _ = graceful.shutdown() => {}
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            logger.print("shutdown: timed out");
        }
    }
}

async fn handle_with_read_timeout(
    server: Arc<Server>,
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    match tokio::time::timeout(Duration::from_secs(15), server.handle_incoming(req)).await {
        Ok(resp) => resp,
        Err(_) => timeout_response(),
    }
}

fn timeout_response() -> Response<Full<Bytes>> {
    Response::builder()
        .status(hyper::StatusCode::REQUEST_TIMEOUT)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(hyper::header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Full::new(Bytes::from_static(b"timeout\n")))
        .unwrap()
}

fn normalize_listen_addr(addr: &str) -> String {
    if addr.starts_with(':') {
        format!("[::]{addr}")
    } else {
        addr.to_string()
    }
}
