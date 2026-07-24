use httpr::{Data, Method, Router, listen_and_serve};
use std::{
    net::TcpListener,
    sync::{Arc, atomic::AtomicI64},
};

struct App {
    count: AtomicI64,
}

fn main() {
    let ln = TcpListener::bind("127.0.0.1:8080").unwrap();

    let app = App {
        count: AtomicI64::new(0),
    };

    let router = Router::new().add(Method::GET, "/", |ctx: &Arc<App>, _, _| {
        let count = ctx.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        Data::String(format!("Count: {}", count)).into()
    });

    listen_and_serve(ln, Arc::new(app), router);
}
