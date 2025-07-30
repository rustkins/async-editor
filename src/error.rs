use derive_more::From;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, From)]
pub enum Error {
    Msg(&'static str),
    /// [`SharedStdout`] has already dropped
    RedrawRcError,
    SharedStdoutClosed,
    #[from]
    Fmt(std::fmt::Error),
    #[from]
    Io(std::io::Error),
    //#[from]
    //SerdeJson(serde_json::Error)
}

impl core::fmt::Display for Error {
    fn fmt(&self, fmt: &mut core::fmt::Formatter) -> core::result::Result<(), core::fmt::Error> {
        write!(fmt, "{self:?}")
    }
}

impl std::error::Error for Error {}

/*impl From<&'static str> for Error {
    fn from(s: &'static str) -> Self {
        Error::Msg(s)
    }
}


pub fn msg<E: Into<Error>>(msg: &str) -> E {
    move || msg.into()
}*/
//pub fn msg<E: Into<Error>>(msg: &str) -> impl FnOnce() -> E {
//    move || msg.into()
//}
// Need FnOnce
//pub fn msg(msg: &str) -> Error {
//    msg.into()
//}
//pub fn err<E: Into<Error>>(msg: &str) -> impl FnOnce() -> E {
//    move || msg.into()
//}
