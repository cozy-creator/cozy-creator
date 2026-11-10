use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use super::{Catalog, Source};

#[test]
fn package_plan_names_this_machines_cpu() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 1024];
        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            let read = socket.read(&mut buffer).unwrap();
            assert_ne!(read, 0);
            bytes.extend_from_slice(&buffer[..read]);
        }
        let header_end = bytes.windows(4).position(|part| part == b"\r\n\r\n").unwrap() + 4;
        let head = String::from_utf8_lossy(&bytes[..header_end]);
        let length: usize = head.lines().filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse().unwrap()).unwrap_or(0);
        while bytes.len() < header_end + length {
            let read = socket.read(&mut buffer).unwrap();
            assert_ne!(read, 0);
            bytes.extend_from_slice(&buffer[..read]);
        }
        let request = String::from_utf8(bytes).unwrap();
        assert!(request.starts_with("POST "));
        let body = r#"{"release":"1.0","downloads":[],"package_interface":{},"pyproject":{},"uv_lock":{}}"#;
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        request
    });
    let source = Source {
        origin: origin.clone(),
        ca_der: None,
        capability: None,
    };
    let catalog = Catalog::new(&source).unwrap();
    assert_eq!(
        catalog
            .install_plan("author", "example", "1.0", Some("3.12.12"))
            .unwrap()
            .release,
        "1.0"
    );
    let request = server.join().unwrap();
    let path = request
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let url = reqwest::Url::parse(&format!("{origin}{path}")).unwrap();
    assert_eq!(url.path(), "/v1/packages/author/example/download");
    let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(
        query.get("cpu_arch").map(|v| v.as_ref()),
        Some(std::env::consts::ARCH)
    );
    assert_eq!(
        query.get("python_version").map(|v| v.as_ref()),
        Some("3.12.12")
    );
    assert_eq!(query.get("release").map(|v| v.as_ref()), Some("1.0"));
}
