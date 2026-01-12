use clap::Parser;
use thubo::QoS;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Bind TCP listener
    let listener = TcpListener::bind(args.addr).await.unwrap();
    // Accept connection
    let (stream, _addr) = listener.accept().await.unwrap();
    stream.set_nodelay(true).unwrap();
    let (stream_reader, stream_writer) = stream.into_split();

    // Create thubo sender and spawn a sender_task
    let (mut sender, _sender_task) = thubo::sender(stream_writer).build();
    sender.qos(QoS::default().with_express(true));

    // Create thubo receiver and spawn a receiver_task
    let (mut receiver, _receiver_task) = thubo::receiver(stream_reader).build();

    // Echo loop: receive messages and immediately send them back
    loop {
        // Receive incoming message from the ping client
        let (data, _) = receiver.recv().await.unwrap();
        // Echo the message back to the sender
        sender.send(&data).await.unwrap();
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "16")]
    capacity: usize,
    #[arg(short, long, default_value = "65535")]
    batch_size: u16,
}
