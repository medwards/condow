//! # CONcurrent DOWnloads for local files
//!
//! Load parts of files concurrently.
//!
//! This is mostly for testing and experimenting.
//! In most cases it is better to load sequentially from disks.
//!
//! ```rust, noexec
//!
//! use condow_fs::*;
//! use condow_fs::config::Config;
//!
//! # async {
//! let condow = FsClient::condow(Config::default()).unwrap();
//!
//! let location = url::Url::from_file_path("my_file").expect("a valid path");
//!
//! let stream = condow.download(location, 23..46).await.unwrap();
//! let downloaded_bytes: Vec<u8> = stream.into_vec().await.unwrap();
//! # };
//! # ()
//! ```

use std::io::SeekFrom;
use std::path::Path;

use anyhow::Error as AnyError;
use bytes::Bytes;
use condow_core::config::Config;
use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use condow_core::{
    condow_client::{CondowClient, DownloadSpec},
    errors::CondowError,
    streams::{BytesHint, BytesStream},
};

pub use condow_core::*;

#[derive(Clone)]
pub struct FsClient;

impl FsClient {
    /// Create a concurrent downloader from this adapter and the given [Config]
    pub fn condow(config: Config) -> Result<Condow<Self>, AnyError> {
        Condow::new(FsClient, config)
    }
}

impl CondowClient for FsClient {
    fn get_size(&self, location: url::Url) -> BoxFuture<'static, Result<u64, CondowError>> {
        // TODO: use location.to_file_path
        let f = async move {
            let file = fs::File::open(Path::new(location.path()).to_path_buf()).await?;
            let len = file.metadata().await?.len();

            Ok(len)
        };

        Box::pin(f)
    }

    fn download(
        &self,
        location: url::Url,
        spec: DownloadSpec,
    ) -> BoxFuture<'static, Result<(BytesStream, BytesHint), CondowError>> {
        let path = Path::new(location.path()).to_path_buf();
        let f = async move {
            let bytes = match spec {
                DownloadSpec::Complete => fs::read(path).await?,
                DownloadSpec::Range(range) => {
                    let mut file = fs::File::open(path).await?;
                    file.seek(SeekFrom::Start(range.start())).await?;

                    let n_bytes_to_read = range.len();

                    if n_bytes_to_read > usize::MAX as u64 {
                        return Err(CondowError::new_other(
                            "usize overflow while casting from u64",
                        ));
                    }

                    let mut buffer = vec![0; n_bytes_to_read as usize];

                    let n_bytes_read = file.read_exact(&mut buffer).await?;

                    if n_bytes_read as u64 != n_bytes_to_read {
                        return Err(CondowError::new_io(format!(
                            "not enough bytes read (expected {} got {})",
                            n_bytes_to_read, n_bytes_read
                        )));
                    }

                    buffer
                }
            };

            let bytes = Bytes::from(bytes);

            let bytes_hint = BytesHint::new_exact(bytes.len() as u64);

            let stream = futures::stream::once(futures::future::ready(Ok(bytes)));

            Ok((stream.boxed(), bytes_hint))
        };

        Box::pin(f)
    }
}
