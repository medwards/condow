use condow_client::CondowClient;
use config::Config;
use errors::{DownloadFileError, DownloadRangeError};

pub mod condow_client;
pub mod config;
mod download_range;
pub mod errors;
mod machinery;
pub mod streams;

pub use download_range::DownloadRange;
use streams::{BytesStream, ChunkStream, TotalBytesHint};

#[cfg(test)]
pub mod test_utils;

#[derive(Clone)]
pub struct Condow<C> {
    client: C,
    config: Config,
}

impl<C: CondowClient> Condow<C> {
    pub async fn download_file(
        &self,
        location: C::Location,
    ) -> Result<ChunkStream, DownloadFileError> {
        self.download_range(location, DownloadRange::Full)
            .await
            .map_err(DownloadFileError::from)
    }

    pub async fn download_range<R: Into<DownloadRange>>(
        &self,
        location: C::Location,
        range: R,
    ) -> Result<ChunkStream, DownloadRangeError> {
        let mut range: DownloadRange = range.into();
        range.validate()?;
        range.sanitize();

        if range == DownloadRange::Empty {
            return Ok(ChunkStream::empty());
        }

        let size = self.client.get_size(location.clone()).await?;

        if size == 0 {
            return Ok(ChunkStream::empty());
        }

        if size <= self.config.part_size.0 {
            let (bytes_stream, total_bytes_hint) = self
                .download_file_non_concurrent(location)
                .await
                .map_err(DownloadRangeError::from)?;
            return Ok(ChunkStream::from_full_file(bytes_stream, total_bytes_hint));
        }

        if let Some((start, end_incl)) = range.inclusive_boundaries(size) {
            machinery::download(
                self.client.clone(),
                location,
                start,
                end_incl,
                self.config.clone(),
            )
            .await
        } else {
            Ok(ChunkStream::empty())
        }
    }

    pub async fn download_file_non_concurrent(
        &self,
        location: C::Location,
    ) -> Result<(BytesStream, TotalBytesHint), DownloadFileError> {
        self.client
            .download(location, DownloadRange::Full)
            .await
            .map_err(DownloadFileError::from)
    }
}
