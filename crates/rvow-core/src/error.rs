use std::fmt;

/// A failure that stops a command, with a stable code and a fix the user or the
/// agent can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub fix: String,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Error {
        Error {
            code,
            message: message.into(),
            fix: fix.into(),
        }
    }

    pub fn io(path: &std::path::Path, err: std::io::Error) -> Error {
        Error::new(
            "io",
            format!("{}: {err}", path.display()),
            "check that the path exists and is writable",
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} (fix: {})", self.code, self.message, self.fix)
    }
}

impl std::error::Error for Error {}
