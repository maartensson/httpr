mod router;

use http_body_util::Full;
use hyper::{
    Request, Response,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::Service,
};

use smol::Async;
use smol_hyper::rt::{FuturesIo, SmolTimer};
use std::{
    convert::Infallible,
    future::{Ready, ready},
    net::TcpListener,
    sync::Arc,
};

pub use router::Router;

pub fn listen_and_serve<C: Clone + Send + Sync>(ln: TcpListener, ctx: C, router: Router<C>) {
    let rt = smol::Executor::new();
    smol::block_on(rt.run(async {
        let lis = Async::new(ln).unwrap();
        let svc = Arc::new(WebService::new(ctx, router));
        loop {
            let (stream, _) = lis.accept().await.unwrap();
            let svc = svc.clone();
            rt.spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .timer(SmolTimer::new())
                    .serve_connection(FuturesIo::new(stream), svc)
                    .await
                {
                    eprintln!("Error serving connection: {:?}", err)
                }
            })
            .detach();
        }
    }));
}

struct WebService<C: Clone + Send + Sync> {
    ctx: C,
    router: Router<C>,
}

impl<C: Clone + Send + Sync> WebService<C> {
    pub fn new(ctx: C, router: Router<C>) -> Self {
        Self { ctx, router }
    }
}

impl<C: Clone + Send + Sync> Service<Request<Incoming>> for WebService<C> {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        ready(Ok(self.router.handle(&self.ctx, req)))
    }
}
