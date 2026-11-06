use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

use mio::{Interest, Registry, Token, event::Event, net::TcpStream};

use crate::{
    h1,
    response::{bad_request, redirect_to_https},
};

const MAX_HEADERS: usize = 16 * 1024;

struct Connection {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
}

pub struct Server {
    connections: HashMap<Token, Connection>,
    https_port: u16,
}

impl Server {
    pub fn new(https_port: u16) -> Self {
        Self {
            connections: HashMap::new(),
            https_port,
        }
    }

    pub fn insert(
        &mut self,
        registry: &Registry,
        token: Token,
        mut stream: TcpStream,
    ) -> io::Result<()> {
        registry.register(&mut stream, token, Interest::READABLE)?;

        self.connections.insert(
            token,
            Connection {
                stream,
                input: Vec::new(),
                output: Vec::new(),
            },
        );

        Ok(())
    }

    pub fn handle_event(
        &mut self,
        registry: &Registry,
        token: Token,
        event: &Event,
    ) -> io::Result<()> {
        let close = {
            let Some(conn) = self.connections.get_mut(&token) else {
                return Ok(());
            };

            let mut close = false;

            if event.is_readable() && conn.output.is_empty() {
                let mut buf = [0u8; 4096];

                loop {
                    match conn.stream.read(&mut buf) {
                        Ok(0) => {
                            close = true;
                            break;
                        }
                        Ok(n) => {
                            conn.input.extend_from_slice(&buf[..n]);

                            if conn.input.len() > MAX_HEADERS {
                                let response = bad_request();
                                conn.output = h1::serialize_response(response);
                                break;
                            }

                            let mut headers = [httparse::EMPTY_HEADER; 64];
                            let mut req = httparse::Request::new(&mut headers);

                            match req.parse(&conn.input) {
                                Ok(httparse::Status::Complete(_)) => {
                                    let authority = req
                                        .headers
                                        .iter()
                                        .find(|h| h.name.eq_ignore_ascii_case("Host"))
                                        .and_then(|h| std::str::from_utf8(h.value).ok())
                                        .and_then(|h| h.parse::<http::uri::Authority>().ok());

                                    let host =
                                        authority.as_ref().map(|a| a.host()).unwrap_or("localhost");

                                    let location = if self.https_port == 443 {
                                        host.to_string()
                                    } else {
                                        format!("{}:{}", host, self.https_port)
                                    };
                                    let path = req.path.unwrap_or("/");

                                    let response = redirect_to_https(location.as_str(), path);
                                    conn.output = h1::serialize_response(response);
                                    break;
                                }
                                Ok(httparse::Status::Partial) => continue,
                                Err(_) => {
                                    conn.output = h1::serialize_response(bad_request());
                                    break;
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            close = true;
                            break;
                        }
                    }
                }
            }

            if !close && !conn.output.is_empty() && event.is_writable() {
                while !conn.output.is_empty() {
                    match conn.stream.write(&conn.output) {
                        Ok(0) => {
                            close = true;
                            break;
                        }
                        Ok(n) => {
                            conn.output.drain(..n);
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            close = true;
                            break;
                        }
                    }
                }

                if conn.output.is_empty() {
                    close = true;
                }
            }

            if !close {
                let interest = if conn.output.is_empty() {
                    Interest::READABLE
                } else {
                    Interest::WRITABLE
                };

                registry.reregister(&mut conn.stream, token, interest)?;
            }

            close
        };

        if close {
            if let Some(mut conn) = self.connections.remove(&token) {
                registry.deregister(&mut conn.stream)?;
            }
        }

        Ok(())
    }

    pub fn contains(&self, token: Token) -> bool {
        self.connections.contains_key(&token)
    }
}
