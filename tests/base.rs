use std::sync::Arc;

use thubo::Bytes;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Barrier,
};

const ADDR: &str = "localhost:9999";
const N: u64 = 1_000;

async fn pong(barrier: Arc<Barrier>) {
    // Bind TCP listener
    let listener = TcpListener::bind(ADDR).await.unwrap();
    barrier.wait().await;

    // Accept connection
    let (stream, _addr) = listener.accept().await.unwrap();
    let (stream_reader, stream_writer) = stream.into_split();

    let (mut sender, _sender_task) = thubo::sender(stream_writer).build();
    let (mut receiver, _receiver_task) = thubo::receiver(stream_reader).build();

    // Echo loop: receive messages and immediately send them back
    loop {
        // Receive incoming message from the ping client
        let (data, _) = receiver.recv().await.unwrap();
        // Echo the message back to the sender
        sender.send(&data).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base() {
    let barrier = Arc::new(Barrier::new(2));

    tokio::task::spawn(pong(barrier.clone()));
    barrier.wait().await;

    // Connect to TCP socket
    let stream = TcpStream::connect(ADDR).await.unwrap();
    let (stream_reader, stream_writer) = stream.into_split();

    // Create thubo sender and spawn a sender_task
    let (mut sender, _sender_task) = thubo::sender(stream_writer).build();

    // Create thubo receiver and spawn a receiver_task
    let (mut receiver, _receiver_task) = thubo::receiver(stream_reader).build();

    // Measurement phase: collect RTT samples for each ping/pong round trip
    for size in [8, 32_000, 1_000_000] {
        let payload = Bytes::from(vec![0u8; size]);
        for _ in 0..N {
            sender.send(&payload).await.unwrap();
            let _ = receiver.recv().await.unwrap();
        }
    }
}
