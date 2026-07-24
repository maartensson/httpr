mod http;
mod router;
mod service;

pub use http::listen_and_serve;

pub use hyper::Method;
pub use router::Router;

pub enum Data {
    String(String),
    Bytes(Vec<u8>),
}

use http_body_util::Full;
use hyper::{Response, body::Bytes};

impl From<Data> for Response<Full<Bytes>> {
    fn from(value: Data) -> Self {
        match value {
            Data::String(s) => hyper::Response::new(Full::new(Bytes::from(s))),
            Data::Bytes(bs) => hyper::Response::new(Full::new(Bytes::from(bs))),
        }
    }
}
