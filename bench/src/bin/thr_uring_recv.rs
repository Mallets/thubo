#[cfg(target_os = "linux")]
mod linux {
    use std::{
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use clap::Parser;

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse();
        let listener = TcpListener::bind(&args.addr)?;
        let (stream, _) = listener.accept()?;
        let (mut receiver, task) = thubo::uring::receiver(stream)?.capacity(args.capacity).build_uring();
        static MSGS: AtomicUsize = AtomicUsize::new(0);
        tokio::spawn(thubo_bench::stats_loop_recv(
            task,
            Duration::from_secs_f32(args.interval),
            &MSGS,
        ));
        loop {
            receiver.recv().await?;
            MSGS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[derive(Debug, Parser)]
    struct Args {
        #[arg(short, long, default_value = "127.0.0.1:9999")]
        addr: String,
        #[arg(short, long, default_value = "16")]
        capacity: usize,
        #[arg(short, long, default_value = "1.0")]
        interval: f32,
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
