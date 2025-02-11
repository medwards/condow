/// Downloading API with shared request instrumentation
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::{
    condow_client::CondowClient,
    errors::CondowError,
    machinery,
    reader::RandomAccessReader,
    reporter::{CompositeReporter, NoReporting, Reporter, ReporterFactory},
    streams::{ChunkStream, PartStream},
    Condow, DownloadRange, Downloads, GetSizeMode, StreamWithReport,
};

/// A downloading API for instrumented downloading.
///
/// This has multiple methods to download data. The main difference to
/// [Condow] itself is, that per request reporting/instrumentation can be enabled.
/// All methods will always create a [Reporter] and collect data. Even those
/// where an explicit [Reporter] is passed.
///
/// The [ReporterFactory] can act as a "global" metrics collector collecting
/// data from the per request generated [Reporter]s.
pub struct DownloadSession<C: CondowClient, RF: ReporterFactory = NoReporting> {
    /// Mode for handling upper bounds of a range and open ranges
    ///
    /// Default: As configured with [Condow] itself
    /// or the struct this was cloned from
    get_size_mode: GetSizeMode,
    condow: Condow<C>,
    reporter_factory: Arc<RF>,
}

impl<C: CondowClient, RF: ReporterFactory> DownloadSession<C, RF> {
    pub(crate) fn new_with_reporting_arc(condow: Condow<C>, rep_fac: Arc<RF>) -> Self {
        Self {
            condow,
            get_size_mode: GetSizeMode::default(),
            reporter_factory: rep_fac,
        }
    }

    /// Change the behaviour on when to query the file size
    pub fn get_size_mode<T: Into<GetSizeMode>>(mut self, get_size_mode: T) -> Self {
        self.get_size_mode = get_size_mode.into();
        self
    }

    /// Returns a reference to the [ReporterFactory].
    pub fn reporter_factory(&self) -> &RF {
        self.reporter_factory.as_ref()
    }

    /// Download the BLOB/range.
    ///
    /// A [Reporter] will be created internally and be notified
    ///
    /// The parts and the chunks streamed have the same ordering as
    /// within the BLOB/range downloaded.
    pub async fn download<R: Into<DownloadRange>>(
        &self,
        location: url::Url,
        range: R,
    ) -> Result<PartStream<ChunkStream>, CondowError> {
        self.download_chunks(location, range)
            .await
            .and_then(PartStream::from_chunk_stream)
    }

    /// Download the chunks of a BLOB/range as received
    /// from the concurrently downloaded parts.
    ///
    /// A [Reporter] will be created internally and be notified
    ///
    /// The parts and the chunks streamed have no specific ordering.
    /// Chunks of the same part still have the correct ordering as they are
    /// downloaded sequentially.
    pub async fn download_chunks<R: Into<DownloadRange>>(
        &self,
        location: url::Url,
        range: R,
    ) -> Result<ChunkStream, CondowError> {
        let reporter = self.reporter_factory.make(&location);
        machinery::download(&self.condow, location, range, self.get_size_mode, reporter)
            .await
            .map(|o| o.stream)
    }

    /// Download the BLOB/range and report events.
    ///
    /// The [Reporter] is the one that was configured when creating [DownloadSession].
    ///
    /// The parts and the chunks streamed have the same ordering as
    /// within the BLOB/range downloaded.
    pub async fn download_rep<R: Into<DownloadRange>>(
        &self,
        location: url::Url,
        range: R,
    ) -> Result<StreamWithReport<PartStream<ChunkStream>, RF::ReporterType>, CondowError> {
        let reporter = self.reporter_factory.make(&location);
        self.download_wrep(location, range, reporter).await
    }

    /// Download the chunks of a BLOB/range as received
    /// from the concurrently downloaded parts and report events.
    ///
    /// The [Reporter] is the one that was configured when creating [DownloadSession].
    ///
    /// The parts and the chunks streamed have no specific ordering.
    /// Chunks of the same part still have the correct ordering as they are
    /// downloaded sequentially.
    pub async fn download_chunks_rep<R: Into<DownloadRange>>(
        &self,
        location: url::Url,
        range: R,
    ) -> Result<StreamWithReport<ChunkStream, RF::ReporterType>, CondowError> {
        let reporter = self.reporter_factory.make(&location);
        self.download_chunks_wrep(location, range, reporter).await
    }

    /// Download the BLOB/range and report events.
    ///
    /// A [Reporter] has to be passed to the method explicitly.
    /// The given reporter will be returned but a [Reporter] from
    /// the contained [ReporterFactory] will still be created and notified.
    ///
    /// The parts and the chunks streamed have the same ordering as
    /// within the BLOB/range downloaded.
    pub async fn download_wrep<R: Into<DownloadRange>, RRP: Reporter>(
        &self,
        location: url::Url,
        range: R,
        reporter: RRP,
    ) -> Result<StreamWithReport<PartStream<ChunkStream>, RRP>, CondowError> {
        let composite = CompositeReporter(self.reporter_factory.make(&location), reporter);
        self.download_chunks_wrep(location, range, composite)
            .await?
            .part_stream()
            .map(|sr| {
                let StreamWithReport { stream, reporter } = sr;
                StreamWithReport {
                    stream,
                    reporter: reporter.1,
                }
            })
    }

    /// Download the chunks of a BLOB/range as received
    /// from the concurrently downloaded parts and report events.
    ///
    /// A [Reporter] has to be passed to the method explicitly.
    /// The given reporter will be returned but a [Reporter] from
    /// the contained [ReporterFactory] will still be created and notified.
    ///
    /// The parts and the chunks streamed have no specific ordering.
    /// Chunks of the same part still have the correct ordering as they are
    /// downloaded sequentially.
    pub async fn download_chunks_wrep<R: Into<DownloadRange>, RPP: Reporter>(
        &self,
        location: url::Url,
        range: R,
        reporter: RPP,
    ) -> Result<StreamWithReport<ChunkStream, RPP>, CondowError> {
        let composite = CompositeReporter(self.reporter_factory.make(&location), reporter);
        machinery::download(&self.condow, location, range, self.get_size_mode, composite)
            .await
            .map(|sr| {
                let StreamWithReport { stream, reporter } = sr;
                StreamWithReport {
                    stream,
                    reporter: reporter.1,
                }
            })
    }

    /// Get the size of a file at the BLOB at location
    pub async fn get_size(&self, location: url::Url) -> Result<u64, CondowError> {
        self.condow.get_size(location).await
    }

    /// Creates a [RandomAccessReader] for the given location
    ///
    /// The reader will use the configured [ReporterFactory].
    pub async fn reader(
        &self,
        location: url::Url,
    ) -> Result<RandomAccessReader<Self>, CondowError> {
        let length = self.get_size(location.clone()).await?;
        Ok(RandomAccessReader::new_with_length(
            self.clone(),
            location,
            length,
        ))
    }

    /// Creates a [RandomAccessReader] for the given location
    ///
    /// The reader will use the configured [ReporterFactory].
    pub fn reader_with_length(&self, location: url::Url, length: u64) -> RandomAccessReader<Self> {
        let mut me = self.clone();
        me.get_size_mode = GetSizeMode::Required;
        RandomAccessReader::new_with_length(me, location, length)
    }
}

impl<C: CondowClient, RF: ReporterFactory> Clone for DownloadSession<C, RF> {
    fn clone(&self) -> Self {
        Self {
            condow: self.condow.clone(),
            reporter_factory: Arc::clone(&self.reporter_factory),
            get_size_mode: self.get_size_mode,
        }
    }
}

impl<C, RF> Downloads for DownloadSession<C, RF>
where
    C: CondowClient,
    RF: ReporterFactory,
{
    fn download<'a, R: Into<DownloadRange> + Send + Sync + 'static>(
        &'a self,
        location: url::Url,
        range: R,
    ) -> BoxFuture<'a, Result<PartStream<ChunkStream>, CondowError>> {
        Box::pin(self.download(location, range))
    }

    fn download_chunks<'a, R: Into<DownloadRange> + Send + Sync + 'static>(
        &'a self,
        location: url::Url,
        range: R,
    ) -> BoxFuture<'a, Result<ChunkStream, CondowError>> {
        Box::pin(self.download_chunks(location, range))
    }

    fn get_size<'a>(&'a self, location: url::Url) -> BoxFuture<'a, Result<u64, CondowError>> {
        Box::pin(self.get_size(location))
    }

    fn reader_with_length(&self, location: url::Url, length: u64) -> RandomAccessReader<Self> {
        DownloadSession::reader_with_length(self, location, length)
    }
}
