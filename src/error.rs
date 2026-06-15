use crate::err;
use axum::{http::StatusCode, Json};
use serde::Serialize;
use serde_with::skip_serializing_none;
use std::borrow::Cow;
use std::fmt;
use tracing::{debug, error, info, trace, warn};

/// Categorized error codes for different types of failures.  
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "data")]
pub enum ErrorCode {
    Validation,       // invalid request parameters (e.g. bad JSON body)
    NotFound,         // cache entry not found
    Conflict,         // duplicate/already exists (e.g. pattern ID conflict)
    Configuration,    // config loading / parsing failure
    AudioDecodeError, // audio decode error (WAV/MP3 — corrupt file or unsupported format)
    Network,          // connection/timeout errors (no upstream status code available)
    RateLimited,      // rate limit exceeded (429 from backend API)
    Upstream(u16),    // upstream API returned a non-success HTTP status — carries the raw code
    Internal,         // unexpected/unclassified error
    Serialization,
    Unauthorized,
    Io,
}

/// AppError: structured errors with categorization.  
#[derive(Debug, Serialize)]
pub enum AppError {
    Internal(InternalError), // our own errors: structured with code, message, data, caused_by
    External(ExternalError), // foreign errors: wrapped with lossy display + full report
}

#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct InternalError {
    pub message: Cow<'static, str>, // free-form human-readable message
    #[serde(rename = "type")]
    pub code: ErrorCode, // categorized error type (not string)
    pub data: Option<serde_json::Value>, // structured error payload (e.g. pattern_id, file path)
    pub caused_by: Option<Box<AppError>>, // error chaining (inner → outer)
}

#[derive(Debug, Serialize)]
pub struct ExternalError {
    pub code: ErrorCode, // categorized error type (not string)
    pub error: String,   // lossy display of foreign error (short for logging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>, // structured error payload
}

// ============================================================================
// AppError construction & accessors
// ============================================================================

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Internal(e) => write!(f, "{}", e.message),
            AppError::External(e) => write!(f, "{}", e.error),
        }
    }
}

impl std::error::Error for AppError {}

impl AppError {
    #[track_caller]
    pub fn new(code: ErrorCode, msg: impl Into<Cow<'static, str>>) -> Self {
        AppError::Internal(
            // our own errors: structured with code, message, data, caused_by
            InternalError {
                message: msg.into(),
                code,
                data: None,
                caused_by: None,
            },
        )
    }

    #[track_caller]
    pub fn external(
        code: ErrorCode,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        AppError::External(
            // foreign errors: wrapped with lossy display + full report
            ExternalError {
                code,
                error: source.to_string(),
                data: None,
            },
        )
    }

    #[track_caller]
    pub fn with_data<T: Serialize>(mut self, data: T) -> Self {
        let value = serde_json::to_value(data).ok();
        match &mut self {
            AppError::Internal(e) => e.data = value,
            AppError::External(e) => e.data = value,
        }
        self
    }

    #[track_caller]
    pub fn with_source(self, source: impl Into<AppError>) -> Self {
        match self {
            AppError::Internal(mut e) => {
                e.caused_by = Some(Box::new(source.into()));
                AppError::Internal(e)
            }
            AppError::External(_) => self, // external errors can't chain sources
        }
    }

    #[track_caller]
    pub fn with_external(self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        match self {
            AppError::Internal(e) => AppError::Internal(InternalError {
                caused_by: Some(Box::new(AppError::external(e.code, source))),
                ..e
            }),
            AppError::External(_) => self,
        }
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            AppError::Internal(e) => e.code,
            AppError::External(e) => e.code,
        }
    }
    pub fn message(&self) -> Cow<'_, str> {
        match self {
            AppError::Internal(e) => e.message.clone(),
            AppError::External(e) => e.error.clone().into(),
        }
    }
}

impl From<std::io::Error> for AppError {
    #[track_caller]
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let err = match e.kind() {
            ErrorKind::NotFound => ErrorCode::NotFound,
            ErrorKind::PermissionDenied => ErrorCode::Unauthorized,
            ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotConnected => ErrorCode::Network,
            _ => ErrorCode::Io,
        };
        err!(@external err, e)
    }
}

impl From<serde_json::Error> for AppError {
    #[track_caller]
    fn from(e: serde_json::Error) -> Self {
        err!(@external Serialization, e)
    }
}

impl From<reqwest::Error> for AppError {
    #[track_caller]
    fn from(e: reqwest::Error) -> Self {
        let code = if e.is_timeout() || e.is_connect() {
            ErrorCode::Network
        } else if let Some(status) = e.status() {
            ErrorCode::Upstream(status.as_u16())
        } else {
            ErrorCode::Network
        };
        err!(@external code, e)
    }
}

// ============================================================================
// Convenience macros
// ============================================================================

#[macro_export]
macro_rules! err {
    // === Internal rules for chaining modifiers ===

    (@apply $err:expr $(,)?) => { $err };

    (@apply $err:expr, @data: { $($key:ident: $val:expr),* $(,)? } $($rest:tt)*) => {
        $crate::err!(@apply $err.with_data($crate::error::__serde_json::json!({ $(stringify!($key): $val),* })) $($rest)*)
    };

    (@apply $err:expr, @data: $data:expr $(, $($rest:tt)*)?) => {
        $crate::err!(@apply $err.with_data($data) $(, $($rest)*)?)
    };

    (@apply $err:expr, @source: $source:expr $(, $($rest:tt)*)?) => {
        $crate::err!(@apply $err.with_source($source) $(, $($rest)*)?)
    };

    (@apply $err:expr, @external: $source:expr $(, $($rest:tt)*)?) => {
        $crate::err!(@apply $err.with_external($source) $(, $($rest)*)?)
    };

    // === Public rules ===

    // Pure external error (no message, just wrap a foreign error):
    // err!(@external Code, source)
    // err!(@external e.error_code(), e)
    (@external $code:expr, $source:expr) => {{
        #[allow(unused_imports)]
        use $crate::error::ErrorCode::*;
        $crate::error::AppError::external($code, $source)
    }};

    // Pure external with data:
    // err!(@external Code, source, @data: value)
    (@external $code:expr, $source:expr, @data: $data:expr) => {{
        #[allow(unused_imports)]
        use $crate::error::ErrorCode::*;
        $crate::error::AppError::external($code, $source).with_data($data)
    }};

    // Message with modifiers: err!(Code, "msg", @data: x, @source: y)
    ($code:expr, $msg:expr, @$($mods:tt)+) => {{
        #[allow(unused_imports)]
        use $crate::error::ErrorCode::*;
        $crate::err!(@apply
            $crate::error::AppError::new($code, $msg),
            @$($mods)+
        )
    }};

    // Formatted with modifiers: err!(Code, "fmt", arg1, arg2, @data: x)
    ($code:expr, $fmt:expr, $($arg:expr),+, @$($mods:tt)+) => {{
        #[allow(unused_imports)]
        use $crate::error::ErrorCode::*;
        $crate::err!(@apply
            $crate::error::AppError::new($code, format!($fmt, $($arg),+)),
            @$($mods)+
        )
    }};

    // Basic: code + literal message
    ($code:expr, $msg:expr) => {{
        #[allow(unused_imports)]
        use $crate::error::ErrorCode::*;
        $crate::error::AppError::new($code, $msg)
    }};

    // Formatted message (must be last due to greedy matching)
    ($code:expr, $fmt:expr, $($arg:expr),+ $(,)?) => {{
        #[allow(unused_imports)]
        use $crate::error::ErrorCode::*;
        $crate::error::AppError::new($code, format!($fmt, $($arg),+))
    }};
}

#[macro_export]
macro_rules! bail {
    ($($args:tt)+) => {
        return ::core::result::Result::Err($crate::err!($($args)+))
    };
}

#[macro_export]
macro_rules! ensure {
    ($cond:expr, $($args:tt)+) => {
        if !($cond) {
            $crate::bail!($($args)+);
        }
    };
}

#[doc(hidden)]
pub use serde_json as __serde_json;
// ============================================================================
// HTTP Error Conversion (axum integration)
// ============================================================================

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = StatusCode::from(self.code());
        (status, Json(self)).into_response()
    }
}

impl From<ErrorCode> for StatusCode {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::Validation | ErrorCode::Configuration => StatusCode::BAD_REQUEST,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Upstream(status) => {
                StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
            ErrorCode::AudioDecodeError | ErrorCode::Network => StatusCode::BAD_GATEWAY,
            ErrorCode::Serialization | ErrorCode::Internal | ErrorCode::Io => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        }
    }
}

pub type Result<T, E = AppError> = ::core::result::Result<T, E>;

pub trait ResultExt<T, E> {
    #[track_caller]
    fn error(self) -> Self
    where
        E: fmt::Display;
    #[track_caller]
    fn warn(self) -> Self
    where
        E: fmt::Display;
    #[track_caller]
    fn info(self) -> Self
    where
        E: fmt::Display;
    #[track_caller]
    fn debug(self) -> Self
    where
        E: fmt::Display;
    #[track_caller]
    fn trace(self) -> Self
    where
        E: fmt::Display;
    #[track_caller]
    fn log(self) -> Self
    where
        E: fmt::Display;
}

impl<T, E> ResultExt<T, E> for core::result::Result<T, E> {
    #[track_caller]
    fn error(self) -> Self
    where
        E: fmt::Display,
    {
        if let Err(ref e) = self {
            let loc = core::panic::Location::caller();
            error!(error = %e, caller.file = loc.file(), caller.line = loc.line());
        }
        self
    }

    #[track_caller]
    fn warn(self) -> Self
    where
        E: fmt::Display,
    {
        if let Err(ref e) = self {
            let loc = core::panic::Location::caller();
            warn!(error = %e, caller.file = loc.file(), caller.line = loc.line());
        }
        self
    }

    #[track_caller]
    fn info(self) -> Self
    where
        E: fmt::Display,
    {
        if let Err(ref e) = self {
            let loc = core::panic::Location::caller();
            info!(error = %e, caller.file = loc.file(), caller.line = loc.line());
        }
        self
    }

    #[track_caller]
    fn debug(self) -> Self
    where
        E: fmt::Display,
    {
        if let Err(ref e) = self {
            let loc = core::panic::Location::caller();
            debug!(error = %e, caller.file = loc.file(), caller.line = loc.line());
        }
        self
    }

    #[track_caller]
    fn trace(self) -> Self
    where
        E: fmt::Display,
    {
        if let Err(ref e) = self {
            let loc = core::panic::Location::caller();
            trace!(error = %e, caller.file = loc.file(), caller.line = loc.line());
        }
        self
    }

    #[track_caller]
    fn log(self) -> Self
    where
        E: fmt::Display,
    {
        self.info()
    }
}
