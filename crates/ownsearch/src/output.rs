use std::fmt;

#[derive(Debug)]
pub(crate) enum PrintError {
    BrokenPipe,
    Io(std::io::Error),
}

impl PrintError {
    pub(crate) fn is_broken_pipe(&self) -> bool {
        matches!(self, Self::BrokenPipe)
    }
}

impl fmt::Display for PrintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrokenPipe => write!(formatter, "the reader closed the pipe"),
            Self::Io(_) => write!(formatter, "writing to the output stream failed"),
        }
    }
}

impl std::error::Error for PrintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BrokenPipe => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<PrintError> for std::io::Error {
    fn from(error: PrintError) -> Self {
        match error {
            PrintError::BrokenPipe => Self::new(std::io::ErrorKind::BrokenPipe, "broken pipe"),
            PrintError::Io(error) => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn a_broken_pipe_is_named_as_such() {
        assert!(PrintError::BrokenPipe.is_broken_pipe());
        assert!(!PrintError::Io(std::io::Error::other("x")).is_broken_pipe());
        assert_eq!(
            PrintError::BrokenPipe.to_string(),
            "the reader closed the pipe"
        );
    }

    #[test]
    fn every_shape_converts_to_io_error_and_reports_its_source() {
        assert_eq!(
            PrintError::Io(std::io::Error::other("boom")).to_string(),
            "writing to the output stream failed"
        );
        assert!(PrintError::BrokenPipe.source().is_none());
        assert!(
            PrintError::Io(std::io::Error::other("boom"))
                .source()
                .is_some()
        );

        let converted: std::io::Error = PrintError::BrokenPipe.into();
        assert_eq!(converted.kind(), std::io::ErrorKind::BrokenPipe);

        let converted: std::io::Error = PrintError::Io(std::io::Error::other("boom")).into();
        assert_eq!(converted.kind(), std::io::ErrorKind::Other);
    }
}
