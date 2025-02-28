use tokio::sync::broadcast;

pub struct Context {
    pub tx: broadcast::Sender<()>,
}

unsafe impl Send for Context {}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop(&self) {
        let _ = self.tx.send(());
    }

    pub fn recv(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}

impl Default for Context {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1);
        Self { tx }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_context() {
        let ctx = Context::new();
        let mut recv = ctx.recv();
        let mut recv2 = ctx.recv();
        tokio::spawn(async move {
            sleep(Duration::from_millis(10)).await;
            ctx.stop();
            assert!(recv2.recv().await.is_ok());
        });
        assert!(recv.recv().await.is_ok());
    }
}
