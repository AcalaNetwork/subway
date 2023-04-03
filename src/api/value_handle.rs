use tokio::sync::{watch, RwLock};

pub struct ValueHandle<T> {
    inner: RwLock<watch::Receiver<Option<T>>>,
}

impl<T: Clone> ValueHandle<T> {
    pub fn new(value: watch::Receiver<Option<T>>) -> Self {
        Self {
            inner: RwLock::new(value),
        }
    }

    pub async fn read(&self) -> T {
        let read_guard = self.inner.read().await;
        let val = (*read_guard).borrow().to_owned();
        drop(read_guard);

        if let Some(val) = val {
            return val;
        }

        let mut write_guard = self.inner.write().await;

        loop {
            tokio::select! {
                res = write_guard.changed() => {
                    if let Err(e) = res {
                        panic!("Changed channel closed: {}", e);
                    }
                    if let Some(value) = (*write_guard).borrow().to_owned() {
                        return value;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::ValueHandle;
    use std::time::Duration;

    #[tokio::test]
    async fn awaits_value() {
        let (value_tx, value_rx) = tokio::sync::watch::channel::<Option<u32>>(None);

        let value_handle = ValueHandle::new(value_rx);

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            value_tx.send_replace(Some(1));
            tokio::time::sleep(Duration::from_millis(1)).await;
            value_tx.send_replace(None);
            tokio::time::sleep(Duration::from_millis(1)).await;
            value_tx.send_replace(Some(2));
        });

        tokio::spawn(async move {
            assert_eq!(value_handle.read().await, 1);
            // after 2 millis value is none but it will await for next value
            tokio::time::sleep(Duration::from_millis(2)).await;
            assert_eq!(value_handle.read().await, 2);
            assert_eq!(value_handle.read().await, 2);
        })
        .await
        .unwrap();

        handle.await.unwrap();
    }
}
