use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use thubo::{ReceiverStats, ReceiverTask, SenderStats, SenderTask};
use tokio::io::{AsyncRead, AsyncWrite};

pub async fn stats_loop_send<W>(sender_task: SenderTask<W>, interval: Duration, atomic_msgs: &'static AtomicUsize)
where
    W: AsyncWrite,
{
    // Stats reporting loop
    let mut tot_msgs = 0;
    let mut tot_batches = 0;
    let mut tot_bytes = 0;

    tokio::time::sleep(interval).await;
    let mut loop_interval = tokio::time::interval(interval);
    loop {
        loop_interval.tick().await;

        macro_rules! xps {
            ($x:expr) => {
                $x as f32 / interval.as_secs_f32()
            };
        }

        let msgs = atomic_msgs.swap(0, Ordering::Relaxed);
        tot_msgs += msgs;

        let SenderStats { batches, bytes, .. } = sender_task.get_stats();
        let diff_batches = batches - tot_batches;
        let diff_bytes = bytes - tot_bytes;
        let avg_bytes = if diff_batches != 0 {
            diff_bytes / diff_batches
        } else {
            0
        };
        let gbps = (8 * diff_bytes) as f64 / 1_000_000_000.0;

        println!(
            "[{:12}]  {:7} msg/s  {:6} batch/s  {:5} B/batch  {:.3} Gb/s",
            tot_msgs,
            xps!(msgs),
            xps!(diff_batches),
            xps!(avg_bytes),
            xps!(gbps)
        );

        tot_batches = batches;
        tot_bytes = bytes;
    }
}

pub async fn stats_loop_recv<R>(receiver_task: ReceiverTask<R>, interval: Duration, atomic_msgs: &'static AtomicUsize)
where
    R: AsyncRead,
{
    // Stats reporting loop
    let mut tot_msgs = 0;
    let mut tot_batches = 0;
    let mut tot_bytes = 0;

    tokio::time::sleep(interval).await;
    let mut loop_interval = tokio::time::interval(interval);
    loop {
        loop_interval.tick().await;

        macro_rules! xps {
            ($x:expr) => {
                $x as f32 / interval.as_secs_f32()
            };
        }

        let msgs = atomic_msgs.swap(0, Ordering::Relaxed);
        tot_msgs += msgs;

        let ReceiverStats { batches, bytes, .. } = receiver_task.get_stats();

        let diff_batches = batches - tot_batches;
        let diff_bytes = bytes - tot_bytes;
        let avg_bytes = if diff_batches != 0 {
            diff_bytes / diff_batches
        } else {
            0
        };
        let gbps = (8 * diff_bytes) as f64 / 1_000_000_000.0;

        println!(
            "[{:12}]  {:7} msg/s  {:6} batch/s  {:5} B/batch  {:.3} Gb/s",
            tot_msgs,
            xps!(msgs),
            xps!(diff_batches),
            xps!(avg_bytes),
            xps!(gbps)
        );

        tot_batches = batches;
        tot_bytes = bytes;
    }
}
