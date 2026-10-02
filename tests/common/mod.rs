//! A loopback HTTP server for the command-line tests that stand in for a
//! remote service. Debug builds of Press read `PRESS_SIRV_API` and
//! `PRESS_STUDIO_API`, so the binary under test talks to this and nothing
//! reaches the real service. One request per connection, then it closes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

/// Serve `answer(method, target, body) -> (status, reply)` until the test
/// process exits, and return the base URL.
pub fn serve(answer: impl Fn(&str, &str, Vec<u8>) -> (u16, Vec<u8>) + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, target) = (
                parts.next().unwrap_or_default().to_string(),
                parts.next().unwrap_or_default().to_string(),
            );
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let (status, reply) = answer(&method, &target, body);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            let _ = stream.write_all(&reply);
        }
    });
    address
}
