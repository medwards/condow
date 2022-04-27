//! Download enqueued [RangeRequest]s sequentially

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use futures::{
    channel::mpsc::{self, Sender, UnboundedSender},
    StreamExt,
};
use tracing::{debug, debug_span, trace, Instrument, Span};

use crate::{
    condow_client::{CondowClient, DownloadSpec},
    config::ClientRetryWrapper,
    errors::{CondowError, IoError},
    machinery::{range_stream::RangeRequest, DownloadSpanGuard, ProbeInternal},
    probe::Probe,
    streams::{BytesStream, Chunk, ChunkStreamItem},
};

use super::KillSwitch;

/// Downloads equeued parts ([RangeRequest]s) of a download sequentially.
///
/// Spawns a task internally to process the parts to be downloaded one by one.
/// The parts to be downloaded are enqueued in a channel.
///
/// Results are pushed into a channel via the [DownloaderContext].
///
/// Usually one `SequentialDownloader` is created for each level of
/// concurrency.  
pub(crate) struct SequentialDownloader {
    request_sender: Sender<RangeRequest>,
}

impl SequentialDownloader {
    pub fn new<C: CondowClient>(
        client: ClientRetryWrapper<C>,
        location: C::Location,
        buffer_size: usize,
        mut context: DownloaderContext,
    ) -> Self {
        let (request_sender, request_receiver) = mpsc::channel::<RangeRequest>(buffer_size);

        let download_span = context.span().clone();

        tokio::spawn(
            async move {
                let mut request_receiver = Box::pin(request_receiver);
                while let Some(range_request) = request_receiver.next().await {
                    let span = debug_span!(
                        parent: context.span(), "download_part", 
                        part_index = %range_request.part_index,
                        part_range = %range_request.blob_range,
                        part_offset = %range_request.range_offset);

                    if context.kill_switch.is_pushed() {
                        // That failed task should have already sent an error...
                        // ...but we do not want to prove that...
                        context.send_err(CondowError::new_other(
                            "another download task already failed",
                        ));
                        return;
                    }

                    match client
                        .download(
                            location.clone(),
                            DownloadSpec::Range(range_request.blob_range),
                            &context.probe,
                        )
                        .instrument(span.clone())
                        .await
                    {
                        Ok((bytes_stream, _total_bytes)) => {
                            if consume_and_dispatch_bytes(bytes_stream, &mut context, range_request)
                                .instrument(span.clone())
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(err) => {
                            context.probe.part_failed(
                                &err,
                                range_request.part_index,
                                &range_request.blob_range,
                            );
                            context.send_err(err);
                            return;
                        }
                    };
                }
                context.mark_successful();
                drop(context);
            }
            .instrument(download_span),
        );

        SequentialDownloader { request_sender }
    }

    pub fn enqueue(&mut self, req: RangeRequest) -> Result<Option<RangeRequest>, ()> {
        match self.request_sender.try_send(req) {
            Ok(()) => Ok(None),
            Err(err) => {
                if err.is_disconnected() {
                    Err(())
                } else {
                    Ok(Some(err.into_inner()))
                }
            }
        }
    }
}

/// A context to control a [SequentialDownloader]
///
/// This has some logic on drop to do some cleanup and error tracking
///
/// Will abort the download vial the [KillSwitch] if the download of
/// a part was not marked as completed.
pub(crate) struct DownloaderContext {
    started_at: Instant,
    counter: Arc<AtomicUsize>,
    kill_switch: KillSwitch,
    probe: ProbeInternal,
    results_sender: UnboundedSender<ChunkStreamItem>,
    completed: bool,
    /// This must exist for the whole download
    download_span_guard: DownloadSpanGuard,
}

impl DownloaderContext {
    pub fn new(
        results_sender: UnboundedSender<ChunkStreamItem>,
        counter: Arc<AtomicUsize>,
        kill_switch: KillSwitch,
        probe: ProbeInternal,
        started_at: Instant,
        download_span_guard: DownloadSpanGuard,
    ) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self {
            counter,
            probe,
            kill_switch,
            started_at,
            results_sender,
            completed: false,
            download_span_guard,
        }
    }

    pub fn send_chunk(&self, chunk: Chunk) -> Result<(), ()> {
        if self.results_sender.unbounded_send(Ok(chunk)).is_ok() {
            return Ok(());
        }

        self.kill_switch.push_the_button();

        return Err(());
    }

    /// Send an error and mark as completed
    pub fn send_err(&mut self, err: CondowError) {
        let _ = self.results_sender.unbounded_send(Err(err));
        self.completed = true;
        self.kill_switch.push_the_button();
    }

    /// Mark the download as complete if successful
    ///
    /// This must be called upon successful termination of an [SequentialDownloader].
    ///
    /// If the download was not marked complete, an error will be sent when dropped
    /// (a panic is assumed).
    pub fn mark_successful(&mut self) {
        self.completed = true;
    }

    pub fn span(&self) -> &Span {
        self.download_span_guard.span()
    }
}

impl Drop for DownloaderContext {
    fn drop(&mut self) {
        if !self.completed {
            self.kill_switch.push_the_button();

            let err = if std::thread::panicking() {
                self.probe.panic_detected("panic detected in downloader");
                CondowError::new_other("download ended unexpectedly due to a panic")
            } else {
                CondowError::new_other("download ended unexpectetly")
            };
            let _ = self.results_sender.unbounded_send(Err(err));
        }

        let remaining_contexts = self.counter.fetch_sub(1, Ordering::SeqCst);
        trace!(parent: self.download_span_guard.span(), "dropping context ({remaining_contexts} contexts before drop)");

        if self.counter.load(Ordering::SeqCst) == 0 {
            if self.kill_switch.is_pushed() {
                debug!(parent: self.download_span_guard.span(), "download failed");
                self.probe.download_failed(Some(self.started_at.elapsed()))
            } else {
                debug!(parent: self.download_span_guard.span(), "download succeeded");
                self.probe.download_completed(self.started_at.elapsed())
            }
        }
    }
}

/// Read chunks of [Bytes] from a stream and dispatch them
/// as [Chunk]s via the [DownloaderContext].
///
/// The [RangeRequest] is only passed for reporting purposes.
///
/// This function marks the [DownloaderContext] as complete via
/// sending an error only.
///
/// [Bytes]: bytes::bytes
async fn consume_and_dispatch_bytes(
    mut bytes_stream: BytesStream,
    context: &mut DownloaderContext,
    range_request: RangeRequest,
) -> Result<(), ()> {
    let mut chunk_index = 0;
    let mut offset_in_range = 0;
    let mut bytes_received = 0;
    let bytes_expected = range_request.blob_range.len();
    let part_start = Instant::now();
    let mut chunk_start = Instant::now();

    context
        .probe
        .part_started(range_request.part_index, range_request.blob_range);

    while let Some(bytes_res) = bytes_stream.next().await {
        match bytes_res {
            Ok(bytes) => {
                let t_chunk = chunk_start.elapsed();
                chunk_start = Instant::now();
                let n_bytes = bytes.len();
                bytes_received += bytes.len() as u64;

                trace!("received chunk of {} bytes (total {bytes_received} received of {bytes_expected})", bytes.len());

                if bytes_received > bytes_expected {
                    let err = CondowError::new_other(format!(
                        "received more bytes than expected for part {} ({}..={}). expected {}, received {}",
                        range_request.part_index,
                        range_request.blob_range.start(),
                        range_request.blob_range.end_incl(),
                        range_request.blob_range.len(),
                        bytes_received
                    ));
                    context.probe.part_failed(
                        &err,
                        range_request.part_index,
                        &range_request.blob_range,
                    );
                    context.send_err(err);
                    return Err(());
                }

                context.probe.chunk_completed(
                    range_request.part_index,
                    chunk_index,
                    n_bytes,
                    t_chunk,
                );

                context.send_chunk(Chunk {
                    part_index: range_request.part_index,
                    chunk_index,
                    blob_offset: range_request.blob_range.start() + offset_in_range,
                    range_offset: range_request.range_offset + offset_in_range,
                    bytes,
                    bytes_left: bytes_expected - bytes_received,
                })?;
                chunk_index += 1;
                offset_in_range += n_bytes as u64;
            }
            Err(IoError(msg)) => {
                context.probe.part_failed(
                    &CondowError::new_io(msg.clone()),
                    range_request.part_index,
                    &range_request.blob_range,
                );
                context.send_err(CondowError::new_io(msg));
                return Err(());
            }
        }
    }

    context.probe.part_completed(
        range_request.part_index,
        chunk_index,
        bytes_received,
        part_start.elapsed(),
    );

    if bytes_received != bytes_expected {
        let err = CondowError::new_other(format!(
            "received wrong number of bytes for part {} ({}..={}). expected {}, received {}",
            range_request.part_index,
            range_request.blob_range.start(),
            range_request.blob_range.end_incl(),
            range_request.blob_range.len(),
            bytes_received
        ));
        context
            .probe
            .part_failed(&err, range_request.part_index, &range_request.blob_range);
        let _ = context.send_err(err);
        Err(())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{atomic::AtomicUsize, Arc},
        time::Instant,
    };

    use futures::StreamExt;
    use tracing::Span;

    use crate::{
        condow_client::{
            failing_client_simulator::FailingClientSimulatorBuilder, CondowClient, IgnoreLocation,
        },
        config::Config,
        errors::{CondowError, CondowErrorKind},
        machinery::{
            download::{
                sequential::{DownloaderContext, SequentialDownloader},
                KillSwitch,
            },
            range_stream::RangeStream,
            DownloadSpanGuard,
        },
        streams::{BytesHint, Chunk, ChunkStream},
        test_utils::*,
        InclusiveRange,
    };

    #[tokio::test]
    async fn from_0_to_inclusive_range_larger_than_part_size() {
        let client = TestCondowClient::new().max_chunk_size(3);

        for range in [
            InclusiveRange(0, 8),
            InclusiveRange(0, 9),
            InclusiveRange(0, 10),
        ] {
            check(range, client.clone(), 10).await.unwrap()
        }
    }

    #[tokio::test]
    async fn failing_request() {
        let blob = (0u8..100).collect::<Vec<_>>();
        let client = FailingClientSimulatorBuilder::default()
            .blob(blob)
            .chunk_size(100)
            .responses()
            .failure(CondowErrorKind::Other)
            .finish();

        assert!(check(InclusiveRange(0, 99), client, 100).await.is_err());
    }

    async fn check<C: CondowClient<Location = IgnoreLocation>>(
        range: InclusiveRange,
        client: C,
        part_size_bytes: u64,
    ) -> Result<(), CondowError> {
        let config = Config::default()
            .buffer_size(10)
            .buffers_full_delay_ms(0)
            .part_size_bytes(part_size_bytes)
            .max_concurrency(1); // Won't work otherwise

        let bytes_hint = BytesHint::new(range.len(), Some(range.len()));

        let (_n_parts, mut ranges_stream) =
            RangeStream::create(range, config.part_size_bytes.into());

        let (result_stream, results_sender) = ChunkStream::new(bytes_hint);

        let mut downloader = SequentialDownloader::new(
            client.into(),
            IgnoreLocation,
            config.buffer_size.into(),
            DownloaderContext::new(
                results_sender,
                Arc::new(AtomicUsize::new(0)),
                KillSwitch::new(),
                Default::default(),
                Instant::now(),
                DownloadSpanGuard::new(Span::none()),
            ),
        );

        while let Some(next) = ranges_stream.next().await {
            let _ = downloader.enqueue(next).unwrap();
        }

        drop(downloader); // Ends the stream

        let result = result_stream.collect::<Vec<_>>().await;
        let result = result.into_iter().collect::<Result<Vec<_>, _>>()?;

        let total_bytes: u64 = result.iter().map(|c| c.bytes.len() as u64).sum();
        assert_eq!(total_bytes, range.len(), "total_bytes");

        let mut next_range_offset = 0;
        let mut next_blob_offset = range.start();

        result.iter().for_each(|c| {
            let Chunk {
                part_index,
                blob_offset,
                range_offset,
                bytes,
                ..
            } = c;
            assert_eq!(
                *range_offset, next_range_offset,
                "part {}, range_offset: {:?}",
                part_index, range
            );
            assert_eq!(
                *blob_offset, next_blob_offset,
                "part {}, blob_offset: {:?}",
                part_index, range
            );
            next_range_offset += bytes.len() as u64;
            next_blob_offset += bytes.len() as u64;
        });

        Ok(())
    }
}