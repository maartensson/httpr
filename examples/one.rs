use std::io;
use std::sync::{Arc, atomic::AtomicI64};

use bytes::Bytes;
use httpr::{Method, Response};
use httpr::{Router, listen_and_serve};
use mio::net::{TcpListener, UdpSocket};
use tracing::info;

struct App {
    count: AtomicI64,
}

fn main() -> io::Result<()> {
    tracing_subscriber::fmt::init();

    let http = TcpListener::bind("127.0.0.1:8080".parse().unwrap())?;
    let https = TcpListener::bind("127.0.0.1:8443".parse().unwrap())?;
    let http3 = UdpSocket::bind("127.0.0.1:8443".parse().unwrap())?;

    let app = Arc::new(App {
        count: AtomicI64::new(0),
    });

    let router = Router::new()
        .add(Method::GET, "/", |ctx: &Arc<App>, _, _| {
            let count = ctx.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            info!(count, "request received");
            Response::new(Bytes::from(format!("Count: {count}")))
        })
        .add(Method::GET, "/down", |ctx: &Arc<App>, _, _| {
            let count = ctx.count.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
            info!(count, "request received");
            Response::new(Bytes::from(format!("Count: {count}")))
        });

    let https_port = https.local_addr()?.port();

    listen_and_serve(
        http,
        https,
        http3,
        app,
        router,
        "./cert.pem",
        "./key.pem",
        https_port,
    )
}
