use crate::{Router, response};
use boring::ssl::{
    ErrorCode, HandshakeError, MidHandshakeSslStream, SslAcceptor, SslFiletype, SslMethod,
    SslStream,
};
use bytes::Bytes;
use http::{Request, Response, Version};
use mio::{Interest, Registry, Token, event::Event, net::TcpStream};
use std::path::Path;
use std::{
    collections::HashMap,
    io::{self, Write},
};
use tracing::{info, warn};

pub fn load(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<SslAcceptor, io::Error> {
    let mut builder =
        SslAcceptor::mozilla_intermediate(SslMethod::tls()).map_err(io::Error::other)?;

    builder.set_certificate_chain_file(cert)?;
    builder.set_private_key_file(key, SslFiletype::PEM)?;
    builder.check_private_key()?;

    Ok(builder.build())
}

const MAX_REQUEST_SIZE: usize = 1 << 20;
const READ_BUFFER_SIZE: usize = 4096;
const MAX_HEADERS: usize = 64;

enum Tls {
    Handshaking(MidHandshakeSslStream<TcpStream>),
    Established(SslStream<TcpStream>),
}

struct Connection {
    tls: Option<Tls>,
    buffer: Vec<u8>,
    output: Vec<u8>,
}

pub struct Server {
    connections: HashMap<Token, Connection>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        registry: &Registry,
        token: Token,
        stream: TcpStream,
        acceptor: &SslAcceptor,
    ) -> io::Result<()> {
        let mut tls = acceptor.setup_accept(stream).map_err(io::Error::other)?;

        registry.register(tls.get_mut(), token, Interest::READABLE)?;

        self.connections.insert(
            token,
            Connection {
                tls: Some(Tls::Handshaking(tls)),
                buffer: Vec::new(),
                output: Vec::new(),
            },
        );

        Ok(())
    }

    pub fn handle_event<C>(
        &mut self,
        registry: &Registry,
        token: Token,
        event: &Event,
        ctx: &C,
        router: &Router<C, Bytes>,
    ) -> io::Result<()> {
        let close = {
            let Some(conn) = self.connections.get_mut(&token) else {
                return Ok(());
            };

            let mut close = false;

            if matches!(conn.tls.as_ref(), Some(Tls::Handshaking(_))) {
                let Some(Tls::Handshaking(mid)) = conn.tls.take() else {
                    unreachable!();
                };

                match mid.handshake() {
                    Ok(stream) => {
                        conn.tls = Some(Tls::Established(stream));
                    }

                    Err(HandshakeError::WouldBlock(mut mid)) => {
                        let interest = match mid.error().code() {
                            ErrorCode::WANT_READ => Interest::READABLE,
                            ErrorCode::WANT_WRITE => Interest::WRITABLE,
                            _ => unreachable!(),
                        };
                        registry.reregister(mid.get_mut(), token, interest)?;
                        conn.tls = Some(Tls::Handshaking(mid));
                        return Ok(());
                    }

                    Err(HandshakeError::Failure(mid)) => {
                        warn!(?token, error = %mid.error(), "TLS handlshake failed");
                        close = true;
                    }

                    Err(HandshakeError::SetupFailure(e)) => {
                        warn!(?token, %e, "TLS setup failed");
                        close = true;
                    }
                }
            }

            if close {
                true
            } else {
                let Some(Tls::Established(tls)) = conn.tls.as_mut() else {
                    return Ok(());
                };

                let mut interest = Interest::READABLE;

                if event.is_readable() {
                    let mut buffer = [0u8; READ_BUFFER_SIZE];

                    loop {
                        match tls.ssl_read(&mut buffer) {
                            Ok(0) => {
                                close = true;
                                break;
                            }
                            Ok(n) => {
                                conn.buffer.extend_from_slice(&buffer[..n]);

                                if conn.buffer.len() > MAX_REQUEST_SIZE {
                                    conn.buffer.clear();
                                    conn.output = serialize_response(response::bad_request());
                                    interest = Interest::WRITABLE;
                                    break;
                                }

                                match parse_request(&conn.buffer) {
                                    Ok(Some((request, consumed))) => {
                                        conn.buffer.drain(..consumed);
                                        conn.output =
                                            serialize_response(router.handle(ctx, request));
                                        interest = Interest::WRITABLE;
                                        break;
                                    }
                                    Ok(None) => {}
                                    Err(_) => {
                                        conn.buffer.clear();
                                        conn.output = serialize_response(response::bad_request());
                                        interest = Interest::WRITABLE;
                                        break;
                                    }
                                }
                            }
                            Err(e) => match e.code() {
                                ErrorCode::WANT_READ => {
                                    interest = Interest::READABLE;
                                    break;
                                }
                                ErrorCode::WANT_WRITE => {
                                    interest = Interest::WRITABLE;
                                    break;
                                }
                                _ => {
                                    warn!(?token, %e, "TLS read failed");
                                    close = true;
                                    break;
                                }
                            },
                        }
                    }
                }
                if event.is_writable() && !close && !conn.output.is_empty() {
                    while !conn.output.is_empty() {
                        match tls.ssl_write(&conn.output) {
                            Ok(0) => {
                                close = true;
                                break;
                            }
                            Ok(n) => {
                                conn.output.drain(..n);
                            }
                            Err(e) => match e.code() {
                                ErrorCode::WANT_READ => {
                                    interest = Interest::READABLE;
                                    break;
                                }
                                ErrorCode::WANT_WRITE => {
                                    interest = Interest::WRITABLE;
                                }
                                _ => {
                                    warn!(?token, %e, "tcp write failed");
                                    close = true;
                                    break;
                                }
                            },
                        }
                    }

                    if conn.output.is_empty() {
                        close = true;
                    }
                }

                if !close {
                    registry.reregister(tls.get_mut(), token, interest)?;
                }

                close
            }
        };

        if close {
            self.remove(registry, token)?;
        }

        Ok(())
    }

    fn remove(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
        if let Some(mut conn) = self.connections.remove(&token) {
            if let Some(tls) = conn.tls.as_mut() {
                match tls {
                    Tls::Handshaking(mid) => {
                        registry.deregister(mid.get_mut())?;
                    }
                    Tls::Established(stream) => {
                        registry.deregister(stream.get_mut())?;
                    }
                }
            }

            info!(?token, "connection closed");
        }
        Ok(())
    }
}

fn parse_request(input: &[u8]) -> Result<Option<(Request<Bytes>, usize)>, httparse::Error> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Request::new(&mut headers);

    let header_len = match parsed.parse(input)? {
        httparse::Status::Partial => return Ok(None),
        httparse::Status::Complete(n) => n,
    };

    let method = parsed.method.ok_or(httparse::Error::Token)?;
    let path = parsed.path.ok_or(httparse::Error::Token)?;

    let content_length = parsed
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .map(|h| {
            std::str::from_utf8(h.value)
                .map_err(|_| httparse::Error::HeaderValue)
                .and_then(|v| v.parse::<usize>().map_err(|_| httparse::Error::HeaderValue))
        })
        .transpose()?
        .unwrap_or(0);

    // Chunked requests need separate handling.
    if parsed
        .headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("transfer-encoding"))
    {
        return Err(httparse::Error::HeaderValue);
    }

    let total_len = header_len
        .checked_add(content_length)
        .ok_or(httparse::Error::HeaderValue)?;

    if input.len() < total_len {
        return Ok(None);
    }

    let version = match parsed.version.ok_or(httparse::Error::Version)? {
        0 => Version::HTTP_10,
        1 => Version::HTTP_11,
        _ => return Err(httparse::Error::Version),
    };

    let mut builder = Request::builder().method(method).uri(path).version(version);

    {
        let request_headers = builder.headers_mut().ok_or(httparse::Error::HeaderName)?;

        for header in parsed.headers.iter() {
            let name = http::HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|_| httparse::Error::HeaderName)?;

            let value = http::HeaderValue::from_bytes(header.value)
                .map_err(|_| httparse::Error::HeaderValue)?;

            request_headers.append(name, value);
        }
    }

    let body = Bytes::copy_from_slice(&input[header_len..total_len]);

    let request = builder
        .body(body)
        .map_err(|_| httparse::Error::HeaderValue)?;

    Ok(Some((request, total_len)))
}

pub fn serialize_response(mut response: Response<Bytes>) -> Vec<u8> {
    response.headers_mut().insert(
        http::header::ALT_SVC,
        http::HeaderValue::from_static(r#"h3=":8080"; ma=86400"#),
    );

    let (parts, body) = response.into_parts();
    let mut output = Vec::new();

    write!(
        output,
        "HTTP/1.1 {} {}\r\n",
        parts.status.as_u16(),
        parts.status.canonical_reason().unwrap_or("Unknown"),
    )
    .unwrap();

    for (name, value) in &parts.headers {
        write!(output, "{}: ", name).unwrap();
        output.extend_from_slice(value.as_bytes());
        output.extend_from_slice(b"\r\n");
    }

    if !parts.headers.contains_key(http::header::CONTENT_LENGTH) {
        write!(output, "Content-Length: {}\r\n", body.len()).unwrap();
    }

    output.extend_from_slice(b"Connection: close\r\n\r\n");
    output.extend_from_slice(&body);

    output
}
