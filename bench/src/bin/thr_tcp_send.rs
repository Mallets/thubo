use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use clap::Parser;
use thubo::Bytes;
use tokio::net::TcpStream;

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

    // Connect to TCP socket
    let stream = TcpStream::connect(addr).await.unwrap();
    let (_stream_reader, stream_writer) = stream.into_split();

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
    #[arg(short, long, default_value = "127.0.0.1:9999")]
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
