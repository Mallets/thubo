#[cfg(target_os = "linux")]
mod linux {
    use std::{
        net::TcpStream,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use clap::Parser;
    use thubo::Bytes;

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse();
        let stream = TcpStream::connect(&args.addr)?;
        let (mut sender, task) = thubo::uring::sender(stream)?
            .capacity(args.capacity)
            .batch_size(args.batch_size)
            .timeout_batch(Duration::from_secs_f32(args.timeout_batch))
            .build_uring();
        static MSGS: AtomicUsize = AtomicUsize::new(0);
        tokio::spawn(thubo_bench::stats_loop_send(
            task,
            Duration::from_secs_f32(args.interval),
            &MSGS,
        ));
        let bytes = Bytes::from(vec![0x42; args.size]);
        loop {
            sender.send(&bytes).await?;
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
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("io_uring throughput examples require Linux");
    std::process::exit(1);
}
