use crate::Router;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, Uri};
use mio::net::UdpSocket;
use quiche::h3::{self, NameValue};
use rand::Rng;
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    time::{Duration, Instant},
};
use tracing::{debug, warn};

const MAX_REQUEST_SIZE: usize = 1 << 20;
const READ_BUFFER_SIZE: usize = 65_535;
const CONNECTION_ID_LEN: usize = 16;

struct IncomingRequest {
    method: Method,
    uri: Uri,
    //headers: HeaderMap,
    body: Vec<u8>,
}

struct Connection {
    conn: quiche::Connection,
    h3: Option<h3::Connection>,
    peer: SocketAddr,
    deadline: Option<Instant>,
    requests: HashMap<u64, IncomingRequest>,
}

pub struct Server {
    config: quiche::Config,
    h3_config: h3::Config,
    connections: HashMap<Vec<u8>, Connection>,
}

impl Server {
    pub fn new(cert: &str, key: &str) -> io::Result<Self> {
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).map_err(io::Error::other)?;

        config
            .load_cert_chain_from_pem_file(cert)
            .map_err(io::Error::other)?;

        config
            .load_priv_key_from_pem_file(key)
            .map_err(io::Error::other)?;

        config
            .set_application_protos(h3::APPLICATION_PROTOCOL)
            .map_err(io::Error::other)?;

        config.set_max_idle_timeout(30_000);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);

        let h3_config = h3::Config::new().map_err(io::Error::other)?;

        Ok(Self {
            config,
            h3_config,
            connections: HashMap::new(),
        })
    }

    /// Drain incoming UDP datagrams and process them.
    pub fn recv<C>(
        &mut self,
        socket: &UdpSocket,
        ctx: &C,
        router: &Router<C, Bytes>,
    ) -> io::Result<()> {
        let local = socket.local_addr()?;
        let mut buffer = [0u8; READ_BUFFER_SIZE];

        loop {
            let (len, peer) = match socket.recv_from(&mut buffer) {
                Ok(value) => value,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            };

            let header = match quiche::Header::from_slice(&mut buffer[..len], CONNECTION_ID_LEN) {
                Ok(header) => header,
                Err(e) => {
                    debug!(%e, "invalid QUIC packet");
                    continue;
                }
            };

            let mut key = header.dcid.as_ref().to_vec();

            // New QUIC connection: only accept Initial packets.
            if !self.connections.contains_key(&key) {
                if header.ty != quiche::Type::Initial {
                    continue;
                }

                let mut scid = [0u8; CONNECTION_ID_LEN];
                rand::rng().fill(&mut scid);

                let scid = quiche::ConnectionId::from_ref(&scid);
                key = scid.as_ref().to_vec();

                let conn = match quiche::accept(&scid, None, local, peer, &mut self.config) {
                    Ok(conn) => conn,
                    Err(e) => {
                        warn!(%e, %peer, "failed to accept QUIC connection");
                        continue;
                    }
                };

                self.connections.insert(
                    scid.as_ref().to_vec(),
                    Connection {
                        conn,
                        h3: None,
                        peer,
                        deadline: None,
                        requests: HashMap::new(),
                    },
                );

                debug!(%peer, "new QUIC connection");
            }

            // New connections are indexed by the server-issued CID.
            // The first Initial packet is processed directly below.
            let key = if self.connections.contains_key(&key) {
                key
            } else {
                // The packet was an Initial addressed to the client's
                // original destination CID. Find the newly accepted peer.
                match self
                    .connections
                    .iter()
                    .find(|(_, c)| c.peer == peer)
                    .map(|(id, _)| id.clone())
                {
                    Some(id) => id,
                    None => continue,
                }
            };

            let Some(connection) = self.connections.get_mut(&key) else {
                continue;
            };

            let recv_info = quiche::RecvInfo {
                from: peer,
                to: local,
            };

            if let Err(e) = connection.conn.recv(&mut buffer[..len], recv_info) {
                warn!(%e, %peer, "QUIC packet processing failed");
                continue;
            }

            if connection.conn.is_established() && connection.h3.is_none() {
                match h3::Connection::with_transport(&mut connection.conn, &self.h3_config) {
                    Ok(h3) => {
                        connection.h3 = Some(h3);
                        debug!(%peer, "HTTP/3 connection established");
                    }
                    Err(e) => {
                        warn!(%e, %peer, "HTTP/3 initialization failed");
                    }
                }
            }

            if let Some(h3) = connection.h3.as_mut() {
                process_requests(
                    h3,
                    &mut connection.conn,
                    &mut connection.requests,
                    ctx,
                    router,
                );
            }

            flush_packets(socket, connection)?;

            connection.deadline = connection
                .conn
                .timeout()
                .map(|duration| Instant::now() + duration);
        }

        Ok(())
    }

    /// Handle expired QUIC timers.
    pub fn timeout(&mut self, socket: &UdpSocket) -> io::Result<()> {
        let now = Instant::now();

        self.connections.retain(|_, connection| {
            if connection.deadline.is_some_and(|deadline| deadline <= now) {
                connection.conn.on_timeout();
            }

            !connection.conn.is_closed()
        });

        for connection in self.connections.values_mut() {
            flush_packets(socket, connection)?;

            connection.deadline = connection
                .conn
                .timeout()
                .map(|duration| Instant::now() + duration);
        }

        Ok(())
    }

    /// Duration until the next QUIC timer expires.
    pub fn next_timeout(&self) -> Option<Duration> {
        let now = Instant::now();

        self.connections
            .values()
            .filter_map(|connection| connection.deadline)
            .map(|deadline| deadline.saturating_duration_since(now))
            .min()
    }
}

fn process_requests<C>(
    h3: &mut h3::Connection,
    conn: &mut quiche::Connection,
    requests: &mut HashMap<u64, IncomingRequest>,
    ctx: &C,
    router: &Router<C, Bytes>,
) {
    loop {
        let (stream_id, event) = match h3.poll(conn) {
            Ok(event) => event,
            Err(h3::Error::Done) => break,
            Err(e) => {
                warn!(%e, "HTTP/3 processing failed");
                break;
            }
        };

        match event {
            h3::Event::Headers { list, .. } => {
                let mut method = None;
                let mut uri = None;
                let mut headers = HeaderMap::new();

                for header in list {
                    match header.name() {
                        b":method" => {
                            method = Method::from_bytes(header.value()).ok();
                        }
                        b":path" => {
                            uri = std::str::from_utf8(header.value())
                                .ok()
                                .and_then(|value| value.parse::<Uri>().ok());
                        }
                        name if name.starts_with(b":") => {}
                        name => {
                            if let (Ok(name), Ok(value)) = (
                                http::HeaderName::from_bytes(name),
                                http::HeaderValue::from_bytes(header.value()),
                            ) {
                                headers.append(name, value);
                            }
                        }
                    }
                }

                if let (Some(method), Some(uri)) = (method, uri) {
                    requests.insert(
                        stream_id,
                        IncomingRequest {
                            method,
                            uri,
                            //headers,
                            body: Vec::new(),
                        },
                    );
                }
            }

            h3::Event::Data => {
                let Some(request) = requests.get_mut(&stream_id) else {
                    continue;
                };

                let mut buffer = [0u8; 4096];

                loop {
                    match h3.recv_body(conn, stream_id, &mut buffer) {
                        Ok(0) | Err(h3::Error::Done) => break,
                        Ok(n) => {
                            if request.body.len() + n > MAX_REQUEST_SIZE {
                                requests.remove(&stream_id);
                                break;
                            }

                            request.body.extend_from_slice(&buffer[..n]);
                        }
                        Err(e) => {
                            warn!(%e, stream_id, "HTTP/3 body read failed");
                            requests.remove(&stream_id);
                            break;
                        }
                    }
                }
            }

            h3::Event::Finished => {
                let Some(request) = requests.remove(&stream_id) else {
                    continue;
                };

                let request = Request::builder()
                    .method(request.method)
                    .uri(request.uri)
                    .version(http::Version::HTTP_3)
                    .body(Bytes::from(request.body));

                let Ok(request) = request else {
                    continue;
                };

                let response = router.handle(ctx, request);
                send_response(h3, conn, stream_id, response);
            }

            h3::Event::Reset(_) => {
                requests.remove(&stream_id);
            }

            h3::Event::PriorityUpdate | h3::Event::GoAway => {}
        }
    }
}

fn send_response(
    h3: &mut h3::Connection,
    conn: &mut quiche::Connection,
    stream_id: u64,
    response: http::Response<Bytes>,
) {
    let (parts, body) = response.into_parts();

    let status = parts.status.as_u16().to_string();
    let mut headers = vec![h3::Header::new(b":status", status.as_bytes())];

    for (name, value) in &parts.headers {
        // HTTP/2 and HTTP/3 prohibit connection-specific headers.
        if matches!(
            name.as_str(),
            "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade"
        ) {
            continue;
        }

        headers.push(h3::Header::new(name.as_str().as_bytes(), value.as_bytes()));
    }

    if !parts.headers.contains_key(http::header::CONTENT_LENGTH) {
        headers.push(h3::Header::new(
            b"content-length",
            body.len().to_string().as_bytes(),
        ));
    }

    let fin = body.is_empty();

    if let Err(e) = h3.send_response(conn, stream_id, &headers, fin) {
        warn!(%e, stream_id, "HTTP/3 response headers failed");
        return;
    }

    if !body.is_empty() {
        match h3.send_body(conn, stream_id, &body, true) {
            Ok(n) if n == body.len() => {}
            Ok(n) => warn!(
                stream_id,
                written = n,
                total = body.len(),
                "partial HTTP/3 response body"
            ),
            Err(e) => warn!(%e, stream_id, "HTTP/3 response body failed"),
        }
    }
}

fn flush_packets(socket: &UdpSocket, connection: &mut Connection) -> io::Result<()> {
    let mut buffer = [0u8; 1350];

    loop {
        let (len, info) = match connection.conn.send(&mut buffer) {
            Ok(packet) => packet,
            Err(quiche::Error::Done) => break,
            Err(e) => {
                warn!(%e, peer = %connection.peer, "QUIC packet generation failed");
                break;
            }
        };

        match socket.send_to(&buffer[..len], info.to) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // A production implementation must queue this datagram
                // and retry it when the socket becomes writable.
                return Err(e);
            }
            Err(e) => return Err(e),
        }
    }

    Ok(())
}
