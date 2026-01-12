use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use clap::Parser;
use rustls::{RootCertStore, pki_types::ServerName};
use rustls_pemfile::Item;
use thubo::Bytes;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

#[tokio::main]
async fn main() {
    let Args {
        size,
        addr,
        capacity,
        batch_size,
        interval,
        timeout_batch,
    } = Args::parse();

    // TLS config
    let mut root_cert_store = RootCertStore::empty();
    let (item, _) = rustls_pemfile::read_one_from_slice(SERVER_CA.as_bytes())
        .unwrap()
        .unwrap();
    let cert = match item {
        Item::X509Certificate(cert) => cert,
        unknown => unreachable!("{:?}", unknown),
    };
    root_cert_store.add(cert).unwrap();

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    // Connect to TCP socket
    let stream = TcpStream::connect(addr.as_str()).await.unwrap();

    // Connect TLS
    let domain = addr.split_once(':').unwrap().0;
    let domain = ServerName::try_from(domain).unwrap().to_owned();
    let stream = connector.connect(domain, stream).await.unwrap();
    let (_stream_reader, stream_writer) = tokio::io::split(stream);

    // Create thubo sender and spawn a sender_task
    let (mut sender, sender_task) = thubo::sender(stream_writer)
        .capacity(capacity)
        .batch_size(batch_size)
        .timeout_batch(Duration::from_secs_f32(timeout_batch))
        .build();

    static MSGS: AtomicUsize = AtomicUsize::new(0);
    tokio::spawn(thubo_bench::stats_loop_send(
        sender_task,
        Duration::from_secs_f32(interval),
        &MSGS,
    ));

    let bytes = Bytes::from(vec![0x42u8; size]);
    loop {
        sender.send(&bytes).await.unwrap();
        MSGS.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long)]
    size: usize,
    #[arg(short, long, default_value = "localhost:9999")]
    addr: String,
    #[arg(short, long, default_value = "16")]
    capacity: usize,
    #[arg(short, long, default_value = "65535")]
    batch_size: u16,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
    #[arg(short, long, default_value = "0.0001")]
    timeout_batch: f32,
}

// Localhost
const SERVER_CA: &str = "-----BEGIN CERTIFICATE-----
MIIDSzCCAjOgAwIBAgIITcwv1N10nqEwDQYJKoZIhvcNAQELBQAwIDEeMBwGA1UE
AxMVbWluaWNhIHJvb3QgY2EgNGRjYzJmMCAXDTIzMDMwNjE2NDEwNloYDzIxMjMw
MzA2MTY0MTA2WjAgMR4wHAYDVQQDExVtaW5pY2Egcm9vdCBjYSA0ZGNjMmYwggEi
MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQC2WUgN7NMlXIknew1cXiTWGmS0
1T1EjcNNDAq7DqZ7/ZVXrjD47yxTt5EOiOXK/cINKNw4Zq/MKQvq9qu+Oax4lwiV
Ha0i8ShGLSuYI1HBlXu4MmvdG+3/SjwYoGsGaShr0y/QGzD3cD+DQZg/RaaIPHlO
MdmiUXxkMcy4qa0hFJ1imlJdq/6Tlx46X+0vRCh8nkekvOZR+t7Z5U4jn4XE54Kl
0PiwcyX8vfDZ3epa/FSHZvVQieM/g5Yh9OjIKCkdWRg7tD0IEGsaW11tEPJ5SiQr
mDqdRneMzZKqY0xC+QqXSvIlzpOjiu8PYQx7xugaUFE/npKRQdvh8ojHJMdNAgMB
AAGjgYYwgYMwDgYDVR0PAQH/BAQDAgKEMB0GA1UdJQQWMBQGCCsGAQUFBwMBBggr
BgEFBQcDAjASBgNVHRMBAf8ECDAGAQH/AgEAMB0GA1UdDgQWBBTX46+p+Po1npE6
QLQ7mMI+83s6qDAfBgNVHSMEGDAWgBTX46+p+Po1npE6QLQ7mMI+83s6qDANBgkq
hkiG9w0BAQsFAAOCAQEAaN0IvEC677PL/JXzMrXcyBV88IvimlYN0zCt48GYlhmx
vL1YUDFLJEB7J+dyERGE5N6BKKDGblC4WiTFgDMLcHFsMGRc0v7zKPF1PSBwRYJi
ubAmkwdunGG5pDPUYtTEDPXMlgClZ0YyqSFJMOqA4IzQg6exVjXtUxPqzxNhyC7S
vlgUwPbX46uNi581a9+Ls2V3fg0ZnhkTSctYZHGZNeh0Nsf7Am8xdUDYG/bZcVef
jbQ9gpChosdjF0Bgblo7HSUct/2Va+YlYwW+WFjJX8k4oN6ZU5W5xhdfO8Czmgwk
US5kJ/+1M0uR8zUhZHL61FbsdPxEj+fYKrHv4woo+A==
-----END CERTIFICATE-----";
