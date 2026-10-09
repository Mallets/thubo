#![cfg(all(feature = "io-uring", target_os = "linux", not(miri)))]

use std::{
    io::Write,
    net::{Shutdown, TcpListener, TcpStream},
    time::Duration,
};

use thubo::{Bytes, Receiver, ReceiverTask, Sender, SenderTask};

fn available() -> bool {
    match io_uring::IoUring::new(8) {
        Ok(_) => true,
        Err(error) => {
            assert!(
                std::env::var_os("THUBO_REQUIRE_IO_URING").is_none(),
                "io_uring required but unavailable: {error}",
            );
            eprintln!("SKIP: io_uring unavailable: {error}; set THUBO_REQUIRE_IO_URING=1 to require it");
            false
        }
    }
}

fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (client, server)
}

fn tokio_socket(socket: TcpStream) -> tokio::net::TcpStream {
    socket.set_nonblocking(true).unwrap();
    tokio::net::TcpStream::from_std(socket).unwrap()
}

fn payloads() -> Vec<Bytes> {
    let mut chunks = Bytes::new();
    for value in 0..32u8 {
        chunks.push(vec![value; 8192].into());
    }
    vec![
        Bytes::new(),
        Bytes::from(vec![7; 1]),
        Bytes::from(vec![9; 4096]),
        chunks,
        Bytes::from(vec![11; 1_000_000]),
    ]
}

async fn exercise<W, R>(
    mut sender: Sender,
    sender_task: SenderTask<W>,
    mut receiver: Receiver,
    receiver_task: ReceiverTask<R>,
) {
    let transfer_timeout = if std::env::var_os("THUBO_TEST_SLOW").is_some() {
        // Instrumentation changes wall-clock timing, not transport semantics.
        sender_task.set_write_timeout(Duration::from_secs(60));
        receiver_task.set_read_timeout(Duration::from_secs(60));
        Duration::from_secs(60)
    } else {
        Duration::from_secs(5)
    };
    let expected = payloads();
    let sent = expected.clone();
    let producer = tokio::spawn(async move {
        for payload in sent {
            sender.send(&payload).await.unwrap();
        }
        sender.stop().await;
    });
    for payload in expected {
        let (received, _) = tokio::time::timeout(transfer_timeout, receiver.recv())
            .await
            .unwrap_or_else(|e| panic!("receiving {} bytes: {e}", payload.len()))
            .unwrap();
        assert_eq!(received.to_vec(), payload.to_vec());
    }
    producer.await.unwrap();
    #[cfg(feature = "stats")]
    {
        let received = receiver_task.get_stats();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let sent = sender_task.get_stats();
                if sent.batches == received.batches && sent.bytes == received.bytes {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("TX/RX framed byte counters must agree, including drained batches");
        assert!(received.batches > 0);
        assert_eq!(received.dropped, 0);
    }
    // Completion transports return owned batches before their task can finish.
    drop(
        tokio::time::timeout(Duration::from_secs(5), sender_task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
    receiver.stop().await;
    drop(
        tokio::time::timeout(Duration::from_secs(1), receiver_task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
}

#[tokio::test]
async fn uring_to_tokio() {
    if !available() {
        return;
    }
    let (tx, rx) = sockets();
    let (sender, task) = thubo::uring::sender(tx)
        .unwrap()
        .batch_size(1024)
        .capacity(2)
        .build_uring();
    let (receiver, reader_task) = thubo::receiver(tokio_socket(rx)).build();
    exercise(sender, task, receiver, reader_task).await;
}

#[tokio::test]
async fn tokio_to_uring() {
    if !available() {
        return;
    }
    let (tx, rx) = sockets();
    let (sender, task) = thubo::sender(tokio_socket(tx)).batch_size(1024).capacity(2).build();
    let (receiver, reader_task) = thubo::uring::receiver(rx).unwrap().build_uring();
    exercise(sender, task, receiver, reader_task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uring_to_uring() {
    if !available() {
        return;
    }
    let (tx, rx) = sockets();
    let (sender, task) = thubo::uring::sender(tx)
        .unwrap()
        .batch_size(1024)
        .capacity(2)
        .build_uring();
    let (receiver, reader_task) = thubo::uring::receiver(rx).unwrap().capacity(2).build_uring();
    exercise(sender, task, receiver, reader_task).await;
}

#[tokio::test]
async fn partial_reads_keep_alive_and_eof() {
    if !available() {
        return;
    }
    let (mut peer, rx) = sockets();
    let (mut receiver, task) = thubo::uring::receiver(rx).unwrap().build_uring();
    // Keep-alive, then a truncated length-prefixed body. No message may escape.
    peer.write_all(&[0, 0, 4]).unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    peer.write_all(&[0, 1, 2]).unwrap();
    peer.shutdown(Shutdown::Write).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .is_err()
    );
    drop(task.stop().await.unwrap());
}

#[tokio::test]
async fn idle_read_timeout_and_cancellation() {
    if !available() {
        return;
    }
    let (_peer, rx) = sockets();
    let (mut receiver, task) = thubo::uring::receiver(rx).unwrap().build_uring();
    task.set_read_timeout(Duration::from_millis(30));
    assert!(
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .is_err()
    );
    drop(task.stop().await.unwrap());

    let (_peer, rx) = sockets();
    let (_receiver, task) = thubo::uring::receiver(rx).unwrap().build_uring();
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(
        tokio::time::timeout(Duration::from_secs(1), task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
}

#[tokio::test]
async fn native_keep_alive() {
    if !available() {
        return;
    }
    use tokio::io::AsyncReadExt;
    let (tx, peer) = sockets();
    let (sender, task) = thubo::uring::sender(tx).unwrap().build_uring();
    task.set_keep_alive(Duration::from_millis(10));
    let mut peer = tokio_socket(peer);
    for _ in 0..2 {
        let mut header = [1; 4];
        tokio::time::timeout(Duration::from_secs(1), peer.read_exact(&mut header))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(header, [0, 0, 0, 0]);
    }
    sender.stop().await;
    drop(task.stop().await.unwrap());
}

#[tokio::test]
async fn batches_larger_than_the_128k_recv_buffer_are_delivered() {
    if !available() {
        return;
    }
    let (tx, rx) = sockets();
    // A 32-bit batch length allows single frames beyond the old 64 KiB cap;
    // this one spans multiple 128 KiB receive buffers.
    let (mut sender, task) = thubo::uring::sender(tx)
        .unwrap()
        .batch_size(256 * 1024)
        .timeout(Duration::from_secs(5))
        .build_uring();
    let (mut receiver, reader_task) = thubo::uring::receiver(rx).unwrap().build_uring();
    let payload = Bytes::from(vec![5; 200_000]);
    sender.send(&payload).await.unwrap();
    sender.stop().await;
    let (received, _) = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.len(), 200_000);
    assert_eq!(received.to_vec(), vec![5; 200_000]);
    receiver.stop().await;
    drop(
        tokio::time::timeout(Duration::from_secs(1), task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
    drop(
        tokio::time::timeout(Duration::from_secs(1), reader_task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
}

#[tokio::test]
async fn oversized_batches_are_rejected_before_sized_allocation() {
    if !available() {
        return;
    }
    let (tx, rx) = sockets();
    let (mut sender, task) = thubo::uring::sender(tx)
        .unwrap()
        .batch_size(8192)
        .timeout(Duration::from_secs(5))
        .build_uring();
    let (mut receiver, reader_task) = thubo::uring::receiver(rx).unwrap().max_batch_size(4096).build_uring();
    // One 4096-byte message already exceeds the receiver's limit once framed.
    sender.send(&Bytes::from(vec![3; 4096])).await.unwrap();
    sender.stop().await;
    // The receiver must reject the frame and terminate rather than allocate.
    let received = tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await;
    assert!(
        received.map(|result| result.is_err()).unwrap_or(false),
        "oversized batch must not be delivered",
    );
    drop(
        tokio::time::timeout(Duration::from_secs(1), task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
    drop(reader_task.stop().await.unwrap());
}

#[tokio::test]
async fn stalled_writer_times_out_without_reusing_in_flight_buffers() {
    if !available() {
        return;
    }
    use std::os::fd::AsRawFd;
    let (tx, _peer) = sockets();
    let size: libc::c_int = 4096;
    // SAFETY: size is a valid int with the supplied length, and tx owns its fd.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                tx.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        },
        0
    );
    let (mut sender, task) = thubo::uring::sender(tx)
        .unwrap()
        .capacity(2)
        .timeout(Duration::from_millis(500))
        .build_uring();
    task.set_write_timeout(Duration::from_millis(30));
    let payload = Bytes::from(vec![42; 8 * 1024 * 1024]);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), sender.send(&payload))
            .await
            .unwrap()
            .is_err()
    );
    sender.stop().await;
    drop(
        tokio::time::timeout(Duration::from_secs(1), task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
}

#[cfg(feature = "stats")]
#[tokio::test]
async fn cancellation_while_receiver_queue_is_full() {
    if !available() {
        return;
    }
    let (tx, rx) = sockets();
    let (mut sender, tx_task) = thubo::sender(tokio_socket(tx))
        .qos(thubo::QoS::default().with_express(true))
        .build();
    let (_receiver, rx_task) = thubo::uring::receiver(rx).unwrap().capacity(1).build_uring();
    for _ in 0..4 {
        sender.send(Bytes::from(vec![42; 64])).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while rx_task.get_stats().batches < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Cancellation must interrupt delivery backpressure as well as socket I/O.
    drop(
        tokio::time::timeout(Duration::from_secs(1), rx_task.stop())
            .await
            .unwrap()
            .unwrap(),
    );
    sender.stop().await;
    drop(tx_task.stop().await.unwrap());
}

#[tokio::test]
async fn setup_errors_are_not_hidden() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let ring_available = io_uring::IoUring::new(8).is_ok();
    assert_eq!(thubo::uring::sender(socket).is_ok(), ring_available);
}
