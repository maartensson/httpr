use std::io;

use crate::Router;
use crate::h1;
use crate::h3;
use crate::redirect;
use bytes::Bytes;
use mio::{
    Events, Interest, Poll, Token,
    net::{TcpListener, UdpSocket},
};
use tracing::info;

const HTTP: Token = Token(0);
const HTTPS: Token = Token(1);
const HTTP3: Token = Token(2);

pub fn listen_and_serve<C>(
    mut http: TcpListener,
    mut https: TcpListener,
    mut http3: UdpSocket,
    ctx: C,
    router: Router<C, Bytes>,
    cert: &str,
    key: &str,
    redirect_port: u16,
) -> std::io::Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);

    let tls = h1::load(cert, key).map_err(std::io::Error::other)?;
    let mut h1 = h1::Server::new();
    let mut h3 = h3::Server::new(cert, key)?;
    let mut redirect = redirect::Server::new(redirect_port);
    let mut next_token = 3;

    poll.registry()
        .register(&mut http, HTTP, Interest::READABLE)?;

    poll.registry()
        .register(&mut https, HTTPS, Interest::READABLE)?;

    poll.registry()
        .register(&mut http3, HTTP3, Interest::READABLE)?;

    loop {
        poll.poll(&mut events, h3.next_timeout())?;

        for event in events.iter() {
            match event.token() {
                HTTP => loop {
                    match http.accept() {
                        Ok((stream, addr)) => {
                            let token = Token(next_token);
                            next_token += 1;

                            info!(?addr, ?token, "HTTP connection accepted");

                            redirect.insert(poll.registry(), token, stream)?;
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e),
                    }
                },
                HTTPS => loop {
                    match https.accept() {
                        Ok((stream, addr)) => {
                            let token = Token(next_token);
                            next_token += 1;

                            info!(?addr, ?token, "tcp connection accepted");

                            h1.insert(poll.registry(), token, stream, &tls)?;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e),
                    }
                },
                HTTP3 => h3.recv(&http3, &ctx, &router)?,
                token => {
                    if redirect.contains(token) {
                        redirect.handle_event(poll.registry(), token, event)?;
                    } else {
                        h1.handle_event(poll.registry(), token, event, &ctx, &router)?;
                    }
                }
            }
        }

        h3.timeout(&http3)?;
    }
}
