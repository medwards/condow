//! Spawns multiple [SequentialDownloader]s to download parts

use std::{
    sync::{atomic::AtomicUsize, Arc},
    time::Instant,
};

use futures::{channel::mpsc::UnboundedSender, Stream, StreamExt};

use crate::{
    condow_client::CondowClient,
    config::{ClientRetryWrapper, Config},
    machinery::range_stream::RangeRequest,
    reporter::Reporter,
    streams::ChunkStreamItem,
};

use super::{
    sequential::{DownloaderContext, SequentialDownloader},
    KillSwitch,
};

pub(crate) struct ConcurrentDownloader<R: Reporter> {
    downloaders: Vec<SequentialDownloader>,
    counter: usize,
    kill_switch: KillSwitch,
    config: Config,
    reporter: R,
}

impl<R: Reporter> ConcurrentDownloader<R> {
    pub fn new<C: CondowClient>(
        n_concurrent: usize,
        results_sender: UnboundedSender<ChunkStreamItem>,
        client: ClientRetryWrapper<C>,
        config: Config,
        location: url::Url,
        reporter: R,
    ) -> Self {
        let started_at = Instant::now();
        let kill_switch = KillSwitch::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let downloaders: Vec<_> = (0..n_concurrent)
            .map(|_| {
                SequentialDownloader::new(
                    client.clone(),
                    location.clone(),
                    config.buffer_size.into(),
                    DownloaderContext::new(
                        results_sender.clone(),
                        Arc::clone(&counter),
                        kill_switch.clone(),
                        reporter.clone(),
                        started_at,
                    ),
                )
            })
            .collect();

        Self {
            downloaders,
            counter: 0,
            kill_switch,
            config,
            reporter,
        }
    }

    pub async fn download(
        &mut self,
        ranges_stream: impl Stream<Item = RangeRequest>,
    ) -> Result<(), ()> {
        self.reporter.download_started();
        let mut ranges_stream = Box::pin(ranges_stream);
        while let Some(mut range_request) = ranges_stream.next().await {
            let mut attempt = 1;

            let buffers_full_delay = self.config.buffers_full_delay_ms.into();
            let n_downloaders = self.downloaders.len();

            loop {
                if attempt % self.downloaders.len() == 0 {
                    self.reporter.queue_full();
                    tokio::time::sleep(buffers_full_delay).await;
                }
                let idx = self.counter + attempt;
                let downloader = &mut self.downloaders[idx % n_downloaders];

                match downloader.enqueue(range_request) {
                    Ok(None) => break,
                    Ok(Some(msg)) => {
                        range_request = msg;
                    }
                    Err(()) => {
                        self.kill_switch.push_the_button();
                        return Err(());
                    }
                }

                attempt += 1;
            }

            self.counter += 1;
        }
        Ok(())
    }
}
