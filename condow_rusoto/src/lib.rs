//! # CONcurrent DOWnloads from AWS S3
//!
//! Download speed from S3 can be significantly improved by
//! downloading parts of the file concurrently. This crate
//! does exactly that.
//!
//! Unlike e.g. the AWS Java SDK this library does not download
//! the parts as uploaded but ranges.
//!
//! ```rust, noexec
//!
//! use condow_rusoto::*;
//! use condow_rusoto::config::Config;
//!
//! # async {
//! let client = S3ClientWrapper::new(Region::default());
//! let condow = client.condow(Config::default()).unwrap();
//!
//! let location = url::Url::parse("s3://my_bucket/my_object").expect("a valid s3 URL");
//!
//! let stream = condow.download(location, 23..46).await.unwrap();
//! let downloaded_bytes: Vec<u8> = stream.into_vec().await.unwrap();
//! # };
//! # ()
//! ```
use std::{
    fmt,
    ops::{Deref, DerefMut},
};

use anyhow::Error as AnyError;
use futures::{future::BoxFuture, stream::TryStreamExt};
use rusoto_core::{request::BufferedHttpResponse, RusotoError};
use rusoto_s3::{GetObjectError, GetObjectRequest, HeadObjectError, HeadObjectRequest, S3};

pub use rusoto_core::Region;
pub use rusoto_s3::S3Client;

use condow_core::{
    condow_client::*,
    config::Config,
    errors::{CondowError, IoError},
    streams::{BytesHint, BytesStream},
};

pub use condow_core::*;

/// S3 bucket name
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bucket(String);

impl Bucket {
    pub fn new<T: Into<String>>(bucket: T) -> Self {
        Self(bucket.into())
    }

    pub fn object<O: Into<ObjectKey>>(self, key: O) -> S3Location {
        S3Location(self, key.into())
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for Bucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Bucket {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl Deref for Bucket {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Bucket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// S3 object key
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn new<T: Into<String>>(key: T) -> Self {
        Self(key.into())
    }

    pub fn in_bucket<B: Into<Bucket>>(self, bucket: B) -> S3Location {
        S3Location(bucket.into(), self)
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for ObjectKey {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ObjectKey {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<&str> for ObjectKey {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Full "path" to an S3 object
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct S3Location(Bucket, ObjectKey);

impl S3Location {
    pub fn new<B: Into<Bucket>, O: Into<ObjectKey>>(bucket: B, key: O) -> Self {
        Self(bucket.into(), key.into())
    }

    pub fn bucket(&self) -> &Bucket {
        &self.0
    }

    pub fn key(&self) -> &ObjectKey {
        &self.1
    }

    /// Turn this into its two components
    pub fn into_inner(self) -> (Bucket, ObjectKey) {
        (self.0, self.1)
    }
}

impl fmt::Display for S3Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s3://{}/{}", self.0, self.1)
    }
}

/// Just a wrapper around a clietn
/// to implement the trait [CondowClient](condow_client::CondowClient) on.
#[derive(Clone)]
pub struct S3ClientWrapper<C>(C);

impl S3ClientWrapper<S3Client> {
    /// Create a new wrapper wrapping the default [S3Client](rusoto_s3::S3Client)
    /// for the given [Region](rusoto_core::Region).
    pub fn new(region: Region) -> Self {
        let client = S3Client::new(region);
        Self::from_client(client)
    }
}

impl<C: S3 + Clone + Send + Sync + 'static> S3ClientWrapper<C> {
    /// Create a new wrapper wrapping given an implementor of [S3](rusoto_s3::S3).
    pub fn from_client(client: C) -> Self {
        Self(client)
    }

    /// Create a concurrent downloader from this adapter and the given [Config]
    pub fn condow(self, config: Config) -> Result<Condow<Self>, AnyError> {
        Condow::new(self, config)
    }
}

impl<C: S3 + Clone + Send + Sync + 'static> CondowClient for S3ClientWrapper<C> {
    fn get_size(&self, location: url::Url) -> BoxFuture<'static, Result<u64, CondowError>> {
        let client = self.0.clone();
        let bucket = location.host_str().expect("a valid S3 URL").to_string();
        let object_key = location.path().to_string();
        let f = async move {
            let head_object_request = HeadObjectRequest {
                bucket: bucket,
                key: object_key,
                ..Default::default()
            };

            let response = client
                .head_object(head_object_request)
                .await
                .map_err(head_obj_err_to_get_size_err)?;

            if let Some(size) = response.content_length {
                Ok(size as u64)
            } else {
                Err(CondowError::new_other("response had no content length"))
            }
        };

        Box::pin(f)
    }

    fn download(
        &self,
        location: url::Url,
        spec: DownloadSpec,
    ) -> BoxFuture<'static, Result<(BytesStream, BytesHint), CondowError>> {
        let client = self.0.clone();
        let bucket = location.host_str().expect("a valid S3 URL").to_string();
        let object_key = location.path().to_string();
        let f = async move {
            let get_object_request = GetObjectRequest {
                bucket: bucket,
                key: object_key,
                range: spec.http_range_value(),
                ..Default::default()
            };

            let response = client
                .get_object(get_object_request)
                .await
                .map_err(get_obj_err_to_download_err)?;

            let bytes_hint = response
                .content_length
                .map(|s| BytesHint::new_exact(s as u64))
                .unwrap_or_else(BytesHint::new_no_hint);

            let stream = if let Some(stream) = response.body {
                stream
            } else {
                return Err(CondowError::new_other("response had no body"));
            };

            let stream: BytesStream = Box::pin(stream.map_err(|err| IoError(err.to_string())));

            Ok((stream, bytes_hint))
        };

        Box::pin(f)
    }
}

fn get_obj_err_to_download_err(err: RusotoError<GetObjectError>) -> CondowError {
    match err {
        RusotoError::Service(err) => match err {
            GetObjectError::NoSuchKey(s) => CondowError::new_not_found(s),
            GetObjectError::InvalidObjectState(s) => {
                CondowError::new_other(format!("invalid object state (get object request): {}", s))
            }
        },
        RusotoError::Validation(cause) => {
            CondowError::new_other(format!("validation error (get object request): {}", cause))
        }
        RusotoError::Credentials(err) => {
            CondowError::new_other(format!("credentials error (get object request): {}", err))
                .with_source(err)
        }
        RusotoError::HttpDispatch(dispatch_error) => CondowError::new_other(format!(
            "http dispatch error (get object request): {}",
            dispatch_error
        ))
        .with_source(dispatch_error),
        RusotoError::ParseError(cause) => {
            CondowError::new_other(format!("parse error (get object request): {}", cause))
        }
        RusotoError::Unknown(response) => response_to_condow_err(response),
        RusotoError::Blocking => {
            CondowError::new_other("failed to run blocking future within rusoto")
        }
    }
}

fn head_obj_err_to_get_size_err(err: RusotoError<HeadObjectError>) -> CondowError {
    match err {
        RusotoError::Service(err) => match err {
            HeadObjectError::NoSuchKey(s) => CondowError::new_not_found(s),
        },
        RusotoError::Validation(cause) => {
            CondowError::new_other(format!("validation error (head object request): {}", cause))
        }
        RusotoError::Credentials(err) => {
            CondowError::new_other(format!("credentials error (head object request): {}", err))
                .with_source(err)
        }
        RusotoError::HttpDispatch(dispatch_error) => CondowError::new_other(format!(
            "http dispatch error (head object request): {}",
            dispatch_error
        ))
        .with_source(dispatch_error),
        RusotoError::ParseError(cause) => {
            CondowError::new_other(format!("parse error (head object request): {}", cause))
        }
        RusotoError::Unknown(response) => response_to_condow_err(response),
        RusotoError::Blocking => {
            CondowError::new_other("failed to run blocking future within rusoto")
        }
    }
}

fn response_to_condow_err(response: BufferedHttpResponse) -> CondowError {
    let message = if let Ok(body_str) = std::str::from_utf8(response.body.as_ref()) {
        body_str
    } else {
        "<<< response body received from AWS not UTF-8 >>>"
    };

    let status = response.status;
    let message = format!("{} - {}", status, message);
    match status.as_u16() {
        404 => CondowError::new_not_found(message),
        401 | 403 => CondowError::new_access_denied(message),
        _ => {
            if status.is_server_error() {
                CondowError::new_remote(message)
            } else {
                CondowError::new_other(message)
            }
        }
    }
}
