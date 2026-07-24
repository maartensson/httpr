use smol::Async;
use std::{net::TcpListener, sync::Arc};

use crate::router::Router;
use crate::service::WebService;

pub fn listen_and_serve<C: Clone + Send + Sync>(ln: TcpListener, ctx: C, router: Router<C>) {
    let rt = smol::Executor::new();
    smol::block_on(rt.run(async {
        let lis = Async::new(ln).unwrap();
        let svc = Arc::new(WebService::new(ctx, router));
        loop {
            let (stream, _) = lis.accept().await.unwrap();
            let svc = svc.clone();
            rt.spawn(async move { svc.handle(stream).await }).detach();
        }
    }));
}
