use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use thubo::{ReceiverTask, SenderTask};

pub async fn stats_loop_send<W>(sender_task: SenderTask<W>, interval: Duration, atomic_msgs: &'static AtomicUsize) {
    stats_loop(interval, atomic_msgs, move || {
        let stats = sender_task.get_stats();
        (stats.batches, stats.bytes)
    })
    .await;
}

pub async fn stats_loop_recv<R>(receiver_task: ReceiverTask<R>, interval: Duration, atomic_msgs: &'static AtomicUsize) {
    stats_loop(interval, atomic_msgs, move || {
        let stats = receiver_task.get_stats();
        (stats.batches, stats.bytes)
    })
    .await;
}

async fn stats_loop(interval: Duration, atomic_msgs: &'static AtomicUsize, mut sample: impl FnMut() -> (usize, usize)) {
    let mut total_msgs = 0;
    let mut previous_batches = 0;
    let mut previous_bytes = 0;
    let mut previous_time = Instant::now();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let now = Instant::now();
        let seconds = now.duration_since(previous_time).as_secs_f64();
        let msgs = atomic_msgs.swap(0, Ordering::Relaxed);
        total_msgs += msgs;
        let (batches, bytes) = sample();
        let diff_batches = batches - previous_batches;
        let diff_bytes = bytes - previous_bytes;
        println!(
            "[{:12}]  {:12.0} msg/s  {:12.0} batch/s  {:7} B/batch  {:.3} Gb/s",
            total_msgs,
            msgs as f64 / seconds,
            diff_batches as f64 / seconds,
            diff_bytes.checked_div(diff_batches).unwrap_or(0),
            8.0 * diff_bytes as f64 / seconds / 1_000_000_000.0,
        );
        previous_batches = batches;
        previous_bytes = bytes;
        previous_time = now;
    }
}
