use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
};

pub type Param<'a> = (&'a str, &'a str);

pub struct Params<'a> {
    inner: Vec<Param<'a>>,
}

impl<'a> Params<'a> {
    pub fn new(inner: Vec<Param<'a>>) -> Self {
        Self { inner }
    }
    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.inner.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

type Handler<C> = for<'a> fn(&C, &Request<Incoming>, Params<'a>) -> Response<Full<Bytes>>;

struct Route<C> {
    method: Method,
    pattern: &'static str,
    handler: Handler<C>,
}

pub struct Router<C> {
    routes: Vec<Route<C>>,
}

impl<C> Router<C> {
    pub fn new() -> Self {
        Self { routes: vec![] }
    }

    pub fn add(mut self, method: Method, pattern: &'static str, handler: Handler<C>) -> Self {
        self.routes.push(Route {
            method,
            pattern,
            handler,
        });
        self
    }

    pub fn handle(&self, ctx: &C, req: Request<Incoming>) -> Response<Full<Bytes>> {
        let path = req.uri().path();
        let method = req.method();

        // Handle HEAD for all GET routes
        let effective_method = match method {
            &Method::HEAD => &Method::GET,
            m => m,
        };

        for r in &self.routes {
            if &r.method == effective_method {
                if let Some(params) = match_route(r.pattern, path) {
                    let mut response = (r.handler)(&ctx, &req, params);

                    // Strip body if HEAD
                    if method == &Method::HEAD {
                        *response.body_mut() = Full::new(Bytes::new())
                    }

                    return response;
                }
            }
        }
        not_found()
    }
}

fn match_route<'a>(pattern: &'static str, path: &'a str) -> Option<Params<'a>> {
    let mut p = pattern.split('/').filter(|x| !x.is_empty());
    let mut a = path.split('/').filter(|x| !x.is_empty());

    let mut params = Vec::new();

    loop {
        match (p.next(), a.next()) {
            (Some(pp), Some(aa)) => {
                if let Some(name) = pp.strip_prefix(':') {
                    params.push((name, aa));
                    continue;
                }
                if pp != aa {
                    return None;
                }
            }
            (None, None) => return Some(Params::new(params)),
            _ => return None,
        }
    }
}

fn not_found() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Full::new(Bytes::from("not found")))
        .unwrap()
}
