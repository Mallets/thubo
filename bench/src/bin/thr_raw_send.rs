use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use clap::Parser;
use tokio::{io::AsyncWriteExt, net::TcpStream};

#[tokio::main]
async fn main() {
    let Args { size, addr, interval } = Args::parse();

    // Connect to TCP socket
    let stream = TcpStream::connect(addr).await.unwrap();
    let (_stream_reader, mut stream_writer) = stream.into_split();

    static MSGS: AtomicUsize = AtomicUsize::new(0);
    tokio::spawn(async move {
        let mut loop_interval = tokio::time::interval(Duration::from_secs_f32(interval));
        loop {
            loop_interval.tick().await;

            let msgs = MSGS.swap(0, Ordering::Relaxed);
            let gbps = 8.0 * (msgs * size) as f64 / 1_000_000_000.0;

            println!("{:7} msg/s  {:.3} Gb/s", msgs, gbps);
        }
    });

    let msg = vec![0x42u8; size];
    loop {
        stream_writer.write_all(&msg).await.unwrap();
        MSGS.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long)]
    size: usize,
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
}
