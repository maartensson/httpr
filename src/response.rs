use bytes::Bytes;
use http::{StatusCode, header::LOCATION};

pub type Response<B = bytes::Bytes> = http::Response<B>;

pub fn bad_request() -> Response<Bytes> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Bytes::from_static(b"bad request"))
        .unwrap()
}

pub fn not_found() -> Response<Bytes> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Bytes::from_static(b"not found"))
        .unwrap()
}

pub fn redirect_to_https(host: &str, path: &str) -> Response<Bytes> {
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(LOCATION, format!("https://{host}{path}"))
        .body(Bytes::new())
        .unwrap()
}
