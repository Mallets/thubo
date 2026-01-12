use std::time::Duration;

use clap::Parser;
use quanta::Instant;
use thubo::{Bytes, QoS};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let Args {
        size,
        num,
        warmup,
        addr,
        capacity,
        batch_size,
        test,
    } = Args::parse();

    // Connect to TCP socket
    let stream = TcpStream::connect(addr).await.unwrap();
    stream.set_nodelay(true).unwrap();
    let (stream_reader, stream_writer) = stream.into_split();

    // Create thubo sender and spawn a sender_task
    let (mut sender, _sender_task) = thubo::sender(stream_writer)
        .capacity(capacity)
        .batch_size(batch_size)
        .build();
    sender.qos(QoS::default().with_express(true));

    // Create thubo receiver and spawn a receiver_task
    let (mut receiver, _receiver_task) = thubo::receiver(stream_reader).build();

    // Prepare storage for RTT samples and create test message
    let mut samples: Vec<Duration> = Vec::with_capacity(num);
    let bytes = Bytes::from(vec![0x42u8; size]);

    // Warmup phase: run ping/pong cycles to stabilize the system (e.g. cache warming)
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs_f64(warmup) {
        sender.send(&bytes).await.unwrap();
        let _ = receiver.recv().await.unwrap();
    }

    // Measurement phase: collect RTT samples for each ping/pong round trip
    for _ in 0..num {
        let now = Instant::now();
        sender.send(&bytes).await.unwrap();
        let _ = receiver.recv().await.unwrap();
        samples.push(now.elapsed());
    }

    let mut min = Duration::MAX;
    let mut max = Duration::ZERO;
    let mut sum = Duration::ZERO;
    for (seq, rtt) in samples.iter().enumerate() {
        min = min.min(*rtt);
        max = max.max(*rtt);
        sum += *rtt;

        if test {
            println!("{},{},{}", size, seq, rtt.as_nanos());
        } else {
            println!("{} bytes: seq={} rtt={:#.2?} lat={:#.2?}", size, seq, rtt, *rtt / 2);
        }
    }

    let avg = sum / samples.len() as u32;

    if !test {
        println!("rtt min/avg/max = {:#.2?}/{:#.2?}/{:#.2?}", min, avg, max);
        println!("lat min/avg/max = {:#.2?}/{:#.2?}/{:#.2?}", min / 2, avg / 2, max / 2,);
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long)]
    size: usize,
    #[arg(short, long, default_value = "100")]
    num: usize,
    #[arg(short, long, default_value = "1.0")]
    warmup: f64,
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "16")]
    capacity: usize,
    #[arg(short, long, default_value = "65535")]
    batch_size: u16,
    #[arg(short, long)]
    test: bool,
}
