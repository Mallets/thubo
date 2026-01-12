use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use clap::Parser;
use tokio::{io::AsyncReadExt, net::TcpListener};

#[tokio::main]
async fn main() {
    let Args { addr, interval } = Args::parse();

    // Bind TCP listener
    let listener = TcpListener::bind(addr).await.unwrap();
    // Accept connection
    let (stream, _addr) = listener.accept().await.unwrap();
    let (mut stream_reader, _stream_writer) = stream.into_split();

    static BYTES: AtomicUsize = AtomicUsize::new(0);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs_f32(interval));
        loop {
            interval.tick().await;
            let bytes = BYTES.swap(0, Ordering::Relaxed);
            let gbps = (8 * bytes) as f64 / 1_000_000_000.0;
            println!("{:.3} Gb/s", gbps);
        }
    });

    let mut buf = vec![0u8; u16::MAX as usize];
    loop {
        let bytes = stream_reader.read_exact(&mut buf).await.unwrap();
        BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
}
