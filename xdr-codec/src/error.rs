use std::fmt::{Display, Formatter};
use std::string::FromUtf8Error;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Utf8(FromUtf8Error),

    Other(String),

    InvalidCase(i32),
    InvalidEnum(i32),
    InvalidLen(usize),
}

impl Error {
    pub fn invalidcase(v: i32) -> Error {
        Error::InvalidCase(v)
    }

    pub fn invalidenum(v: i32) -> Error {
        Error::InvalidEnum(v)
    }

    pub fn invalidlen(v: usize) -> Error {
        Error::InvalidLen(v)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(err) => write!(f, "io: {}", err),
            Error::Utf8(err) => write!(f, "utf8: {}", err),
            Error::InvalidCase(v) => write!(f, "invalid case: {}", v),
            Error::InvalidLen(v) => write!(f, "invalid len: {}", v),
            Error::InvalidEnum(v) => write!(f, "invalid enum: {}", v),
            Error::Other(s) => write!(f, "other: {}", s),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<FromUtf8Error> for Error {
    fn from(err: FromUtf8Error) -> Self {
        Error::Utf8(err)
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Self::Other(s.into())
    }
}
