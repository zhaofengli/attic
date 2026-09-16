use std::collections::HashSet;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::database::AtticDatabase;

const CHANNEL_CAPACITY: usize = 4096;
const MAX_BATCH_SIZE: usize = 4096;
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub struct AccessTracker {
    sender: mpsc::Sender<i64>,
}

impl AccessTracker {
    pub fn new(database: DatabaseConnection) -> Self {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        tokio::spawn(run_worker(database, receiver));
        Self { sender }
    }

    pub fn record(&self, object_id: i64) {
        if self.sender.try_send(object_id).is_err() {
            tracing::warn!(object_id, "Access tracking queue is full");
        }
    }
}

async fn run_worker(database: DatabaseConnection, mut receiver: mpsc::Receiver<i64>) {
    while let Some(first_id) = receiver.recv().await {
        let object_ids = collect_batch(first_id, &mut receiver, FLUSH_INTERVAL).await;

        let object_ids = object_ids.into_iter().collect::<Vec<_>>();
        if let Err(error) = database.bump_objects_last_accessed(&object_ids).await {
            tracing::error!(object_count = object_ids.len(), %error, "Could not update object access timestamps");
        }
    }
}

async fn collect_batch(
    first_id: i64,
    receiver: &mut mpsc::Receiver<i64>,
    flush_interval: Duration,
) -> HashSet<i64> {
    let mut object_ids = HashSet::from([first_id]);
    let deadline = Instant::now() + flush_interval;

    while object_ids.len() < MAX_BATCH_SIZE {
        tokio::select! {
            object_id = receiver.recv() => match object_id {
                Some(object_id) => {
                    object_ids.insert(object_id);
                }
                None => break,
            },
            _ = sleep_until(deadline) => break,
        }
    }

    object_ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn batch_deduplicates_object_ids() {
        let (sender, mut receiver) = mpsc::channel(8);
        sender.send(7).await.unwrap();
        sender.send(7).await.unwrap();
        sender.send(9).await.unwrap();
        drop(sender);

        let first_id = receiver.recv().await.unwrap();
        let object_ids = collect_batch(first_id, &mut receiver, Duration::from_secs(1)).await;

        assert_eq!(HashSet::from([7, 9]), object_ids);
    }

    #[tokio::test]
    async fn batch_size_is_bounded() {
        let (sender, mut receiver) = mpsc::channel(MAX_BATCH_SIZE + 1);
        for object_id in 0..=MAX_BATCH_SIZE as i64 {
            sender.send(object_id).await.unwrap();
        }

        let first_id = receiver.recv().await.unwrap();
        let object_ids = collect_batch(first_id, &mut receiver, Duration::from_secs(1)).await;

        assert_eq!(MAX_BATCH_SIZE, object_ids.len());
        assert_eq!(Some(MAX_BATCH_SIZE as i64), receiver.recv().await);
    }
}
