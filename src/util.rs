use std::ffi::OsStr;
use std::path::Path;

pub trait OsStrExt {
    fn to_str_or_panic(&self) -> &str;
}

impl OsStrExt for OsStr {
    fn to_str_or_panic(&self) -> &str {
        self.to_str().expect("path should be valid UTF-8")
    }
}

#[expect(unused)]
pub trait PathExt {
    fn to_str_or_panic(&self) -> &str;
}

impl PathExt for Path {
    fn to_str_or_panic(&self) -> &str {
        self.as_os_str().to_str_or_panic()
    }
}
