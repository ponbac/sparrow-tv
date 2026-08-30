use std::{
    env,
    ffi::{OsStr, OsString},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    process,
    time::Duration,
};

const HEALTHCHECK_ARGUMENT: &str = "--healthcheck";
const HEALTHCHECK_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 33_733));
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEALTH_RESPONSE_BYTES: usize = 4 * 1024;
const HEALTH_REQUEST: &[u8] =
    b"GET /health HTTP/1.1\r\nHost: 127.0.0.1:33733\r\nConnection: close\r\n\r\n";
const EXPECTED_HEALTH_BODY: &[u8] = b"{\"status\":\"ok\"}";

#[tokio::main]
async fn main() -> Result<(), sparrow_server::StartupError> {
    match Invocation::parse(env::args_os().skip(1)) {
        Invocation::Serve => sparrow_server::run().await,
        Invocation::Healthcheck => {
            process::exit(if probe_health(ProbeTarget::production()) {
                0
            } else {
                1
            });
        }
        Invocation::Invalid => process::exit(2),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Invocation {
    Serve,
    Healthcheck,
    Invalid,
}

impl Invocation {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Self {
        let mut arguments = arguments.into_iter();
        match (arguments.next(), arguments.next()) {
            (None, None) => Self::Serve,
            (Some(argument), None) if argument == OsStr::new(HEALTHCHECK_ARGUMENT) => {
                Self::Healthcheck
            }
            _ => Self::Invalid,
        }
    }
}

#[derive(Clone, Copy)]
struct ProbeTarget {
    address: SocketAddr,
    timeout: Duration,
}

impl ProbeTarget {
    const fn production() -> Self {
        Self {
            address: HEALTHCHECK_ADDRESS,
            timeout: HEALTHCHECK_TIMEOUT,
        }
    }
}

fn probe_health(target: ProbeTarget) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&target.address, target.timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(target.timeout)).is_err()
        || stream.set_write_timeout(Some(target.timeout)).is_err()
        || stream.write_all(HEALTH_REQUEST).is_err()
    {
        return false;
    }

    let mut response = Vec::with_capacity(MAX_HEALTH_RESPONSE_BYTES);
    if stream
        .take(MAX_HEALTH_RESPONSE_BYTES as u64)
        .read_to_end(&mut response)
        .is_err()
        || response.len() == MAX_HEALTH_RESPONSE_BYTES
    {
        return false;
    }

    is_healthy_response(&response)
}

fn is_healthy_response(response: &[u8]) -> bool {
    let Some(headers_end) = find_bytes(response, b"\r\n\r\n") else {
        return false;
    };
    let headers = &response[..headers_end];
    let body = &response[headers_end + 4..];
    let status_line_end = find_bytes(headers, b"\r\n").unwrap_or(headers.len());
    let mut status_parts = headers[..status_line_end].split(|byte| *byte == b' ');

    status_parts.next() == Some(b"HTTP/1.1".as_slice())
        && status_parts.next() == Some(b"200".as_slice())
        && body == EXPECTED_HEALTH_BODY
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{Ipv4Addr, SocketAddr, TcpListener},
        sync::mpsc,
        thread,
        time::Duration,
    };

    use super::{
        HEALTH_REQUEST, Invocation, MAX_HEALTH_RESPONSE_BYTES, ProbeTarget, is_healthy_response,
        probe_health,
    };

    const TEST_TIMEOUT: Duration = Duration::from_secs(1);

    #[test]
    fn command_line_selects_only_the_exact_healthcheck_mode() {
        assert_eq!(Invocation::parse([]), Invocation::Serve);
        assert_eq!(
            Invocation::parse(["--healthcheck".into()]),
            Invocation::Healthcheck
        );
        assert_eq!(Invocation::parse(["--help".into()]), Invocation::Invalid);
        assert_eq!(
            Invocation::parse(["--healthcheck".into(), "extra".into()]),
            Invocation::Invalid
        );
    }

    #[test]
    fn healthy_response_requires_http_200_and_the_exact_body() {
        assert!(is_healthy_response(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 15\r\n\r\n{\"status\":\"ok\"}"
        ));

        for response in [
            b"HTTP/1.1 503 Service Unavailable\r\n\r\n{\"status\":\"ok\"}".as_slice(),
            b"HTTP/1.0 200 OK\r\n\r\n{\"status\":\"ok\"}".as_slice(),
            b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"stale\"}".as_slice(),
            b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"ok\"}\n".as_slice(),
            b"HTTP/1.1 200 OK\r\ncontent-length: 15\r\n{\"status\":\"ok\"}".as_slice(),
            b"not-http\r\n\r\n{\"status\":\"ok\"}".as_slice(),
        ] {
            assert!(!is_healthy_response(response));
        }
    }

    #[test]
    fn probe_uses_the_fixed_request_and_accepts_a_healthy_response() {
        let (target, server) = response_server(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 15\r\n\r\n{\"status\":\"ok\"}"
                .to_vec(),
        );

        assert!(probe_health(target));
        assert_eq!(server.join().expect("fixture server exits"), HEALTH_REQUEST);
    }

    #[test]
    fn probe_rejects_failure_status_and_extra_body_bytes() {
        for response in [
            b"HTTP/1.1 500 Internal Server Error\r\n\r\n{\"status\":\"ok\"}".to_vec(),
            b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"ok\"}private".to_vec(),
        ] {
            let (target, server) = response_server(response);
            assert!(!probe_health(target));
            server.join().expect("fixture server exits");
        }
    }

    #[test]
    fn probe_rejects_a_response_that_reaches_the_four_kibibyte_cap() {
        let mut response = b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"ok\"}".to_vec();
        response.resize(MAX_HEALTH_RESPONSE_BYTES, b' ');
        let (target, server) = response_server(response);

        assert!(!probe_health(target));
        server.join().expect("fixture server exits");
    }

    #[test]
    fn probe_is_bounded_when_the_server_does_not_finish_responding() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .expect("fixture listener binds to loopback");
        let address = listener.local_addr().expect("fixture address is available");
        let (release_sender, release_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("fixture connection arrives");
            let request = read_health_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                .expect("partial response is writable");
            release_receiver
                .recv_timeout(TEST_TIMEOUT)
                .expect("test releases the held response");
            request
        });
        let target = ProbeTarget {
            address,
            timeout: Duration::from_millis(25),
        };

        assert!(!probe_health(target));
        release_sender.send(()).expect("fixture server is released");
        assert_eq!(server.join().expect("fixture server exits"), HEALTH_REQUEST);
    }

    #[test]
    fn probe_rejects_an_unavailable_loopback_port() {
        assert!(!probe_health(ProbeTarget {
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            timeout: TEST_TIMEOUT,
        }));
    }

    fn response_server(response: Vec<u8>) -> (ProbeTarget, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .expect("fixture listener binds to loopback");
        let address = listener.local_addr().expect("fixture address is available");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("fixture connection arrives");
            let request = read_health_request(&mut stream);
            let _ = stream.write_all(&response);
            request
        });
        (
            ProbeTarget {
                address,
                timeout: TEST_TIMEOUT,
            },
            server,
        )
    }

    fn read_health_request(stream: &mut impl Read) -> Vec<u8> {
        let mut request = vec![0; HEALTH_REQUEST.len()];
        stream
            .read_exact(&mut request)
            .expect("fixture request is readable");
        request
    }
}
