//! Error type shared by the launcher library.

use std::path::PathBuf;

/// Result alias for launcher operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Failure returned by the launcher library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A connection, timeout, or other transport failure.
    #[error("request to {url} failed: {source}")]
    Http {
        /// URL that was requested.
        url: String,
        /// Underlying HTTP client error.
        #[source]
        source: reqwest::Error,
    },

    /// The server answered, but not with a success status.
    #[error("{url} returned HTTP {status}: {body}")]
    HttpStatus {
        /// URL that was requested.
        url: String,
        /// HTTP status code.
        status: u16,
        /// Short excerpt of the response body.
        body: String,
    },

    /// A downloaded file did not match the published SHA-1.
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    Checksum {
        /// File that failed verification.
        path: PathBuf,
        /// Expected SHA-1, lowercase hex.
        expected: String,
        /// SHA-1 that was computed.
        actual: String,
    },

    /// The requested id is not in the version manifest or on disk.
    #[error("version '{id}' was not found")]
    VersionNotFound {
        /// Version id the user asked for.
        id: String,
    },

    /// The resolved version JSON has no client jar.
    #[error("version '{id}' does not include a client jar")]
    MissingClient {
        /// Version id that owns the client download.
        id: String,
    },

    /// JSON from disk or the network could not be decoded.
    #[error("could not parse {context}: {source}")]
    Json {
        /// What was being parsed.
        context: String,
        /// Serde error.
        #[source]
        source: serde_json::Error,
    },

    /// Filesystem failure, with the path that was involved.
    #[error("I/O error at {path}: {source}")]
    Io {
        /// Path that was being created, read, or written.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A library coordinate was not `group:artifact:version[:classifier]`.
    #[error("invalid Maven coordinate '{name}'")]
    Maven {
        /// Raw library name from the version JSON.
        name: String,
    },

    /// No local or downloaded runtime satisfies the version's Java major.
    #[error(
        "no Java {major} runtime found. Install a matching JDK, pass --java, or allow Weathered to download Mojang's runtime"
    )]
    JavaNotFound {
        /// Required major version, such as 21.
        major: u32,
    },

    /// `weathered login` was started without an Azure application id.
    #[error(
        "Microsoft client id is not configured. Set WEATHERED_MS_CLIENT_ID, pass --client-id, or set ms_client_id in the config file"
    )]
    MissingClientId,

    /// Microsoft, Xbox, or Minecraft authentication failed.
    #[error("{0}")]
    Auth(String),

    /// Any other failure with a message that already explains what to do.
    #[error("{0}")]
    Message(String),
}

impl Error {
    /// Filesystem error tied to a path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Error that is already a complete sentence.
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }

    /// Authentication failure that is already a complete sentence.
    pub fn auth(text: impl Into<String>) -> Self {
        Self::Auth(text.into())
    }

    /// JSON error with a short description of the document.
    pub fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Json {
            context: context.into(),
            source,
        }
    }

    /// Whether a download should be tried again.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http { .. } | Self::Checksum { .. } | Self::Io { .. } => true,
            Self::HttpStatus { status, .. } => *status == 408 || *status == 429 || *status >= 500,
            _ => false,
        }
    }
}

/// Adds path context to [`std::io::Result`].
pub trait IoResultExt<T> {
    /// Map an I/O error into [`Error::Io`] using `path`.
    fn io_at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn io_at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
