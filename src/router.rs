use bytes::Bytes;
use http::{Method, Request, Response};

use crate::response;

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

type Handler<C, B> = for<'a> fn(&C, &Request<B>, Params<'a>) -> Response<Bytes>;

struct Route<C, B> {
    method: Method,
    pattern: &'static str,
    handler: Handler<C, B>,
}

pub struct Router<C, B = Bytes> {
    routes: Vec<Route<C, B>>,
}

impl<C, B> Router<C, B> {
    pub fn new() -> Self {
        Self { routes: vec![] }
    }

    pub fn add(mut self, method: Method, pattern: &'static str, handler: Handler<C, B>) -> Self {
        self.routes.push(Route {
            method,
            pattern,
            handler,
        });
        self
    }

    pub fn handle(&self, ctx: &C, req: Request<B>) -> Response<Bytes> {
        let path = req.uri().path();
        let method = req.method();

        let effective_method = if method == Method::HEAD {
            &Method::GET
        } else {
            method
        };

        for route in &self.routes {
            if &route.method != effective_method {
                continue;
            }

            if let Some(params) = match_route(route.pattern, path) {
                let mut response = (route.handler)(ctx, &req, params);

                if method == Method::HEAD {
                    *response.body_mut() = Bytes::new();
                }

                return response;
            }
        }

        response::not_found()
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
