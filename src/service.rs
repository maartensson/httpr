use crate::router::Router;
use http_body_util::Full;
use hyper::{
    Request, Response,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::Service,
};
use smol::io::{AsyncRead, AsyncWrite};
use smol_hyper::rt::{FuturesIo, SmolTimer};
use std::{
    convert::Infallible,
    future::{Ready, ready},
};

pub(crate) struct WebService<C: Clone + Send + Sync> {
    ctx: C,
    router: Router<C>,
}

impl<C: Clone + Send + Sync> WebService<C> {
    pub fn new(ctx: C, router: Router<C>) -> Self {
        Self { ctx, router }
    }

    pub async fn handle<T: AsyncRead + AsyncWrite + Unpin>(&self, stream: T) {
        if let Err(err) = http1::Builder::new()
            .timer(SmolTimer::new())
            .serve_connection(FuturesIo::new(stream), self)
            .await
        {
            eprintln!("Error serving connection: {:?}", err)
        }
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
