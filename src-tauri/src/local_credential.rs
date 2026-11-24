//! The credential the host publishes for this computer's panel (`config/local-auth.token`).
//!
//! The host replaces the file atomically (a temporary file, then `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`), and does it again when the console user changes. A
//! reader that opens the file at the instant of the replace can be refused for a few microseconds: Windows answers ERROR_ACCESS_DENIED while the old file is
//! delete-pending (measured on the test VM: 9 refused reads in 890 000, every one of them read fine at the next attempt). That refusal is a fact about the
//! replace, not about the host, so it is read again here, at once, a few times; nothing else is asked twice. A file that is missing, a user the file does not
//! name, a value that is not a credential are answered as they are, and named in the log (`CredentialError`) so the failure is told apart from a host that
//! refused the connection.
//!
//! The module uses `std` only so its tests also run where the panel itself does not build.

use std::fmt;
use std::path::Path;

/// What stopped the credential from being read. Never carries the file's path or any of its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialFailure {
    /// The file is not there: the host has not published it (not started, or another folder).
    Missing,
    /// The file is there and this user may not read it (the host names another console user until its next check), or the replace still holds it.
    Denied,
    /// Another read error (a sharing violation that does not end, a device error).
    Unreadable,
    /// A file was read and it is not a 64-digit hexadecimal credential (a half-written copy, another file).
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialError {
    pub failure: CredentialFailure,
    /// The operating system's error number, when there is one (Windows: 2 missing, 5 denied, 32 sharing violation).
    pub os_code: Option<i32>,
}

impl CredentialFailure {
    /// The name that stands in the log.
    pub fn label(self) -> &'static str {
        match self {
            CredentialFailure::Missing => "credential-missing",
            CredentialFailure::Denied => "credential-denied",
            CredentialFailure::Unreadable => "credential-unreadable",
            CredentialFailure::Invalid => "credential-invalid",
        }
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.failure {
            CredentialFailure::Invalid => write!(f, "Local host credential is invalid")?,
            CredentialFailure::Missing => write!(f, "Local host credential is unavailable (not published)")?,
            CredentialFailure::Denied => write!(f, "Local host credential is unavailable (access denied)")?,
            CredentialFailure::Unreadable => write!(f, "Local host credential is unavailable (unreadable)")?,
        }
        if let Some(code) = self.os_code {
            write!(f, " [os error {code}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for CredentialError {}

/// How many more times a refused open is asked for, and how long between two asks. The window is microseconds; this is far past it and far under what a person sees.
const REFUSED_RETRIES: u32 = 4;
const REFUSED_PAUSE: std::time::Duration = std::time::Duration::from_millis(1);

fn refused_by_a_replace(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    // ERROR_SHARING_VIOLATION (32) and ERROR_ACCESS_DENIED (5) are what an open sees while the host's replace holds the name
    cfg!(windows) && matches!(error.raw_os_error(), Some(5) | Some(32))
}

fn classify(error: &std::io::Error) -> CredentialError {
    let failure = match error.kind() {
        std::io::ErrorKind::NotFound => CredentialFailure::Missing,
        std::io::ErrorKind::PermissionDenied => CredentialFailure::Denied,
        _ if refused_by_a_replace(error) => CredentialFailure::Denied,
        _ => CredentialFailure::Unreadable,
    };
    CredentialError { failure, os_code: error.raw_os_error() }
}

/// A credential is 32 random bytes in hexadecimal.
pub fn is_credential(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Reads the credential from `path`, asking again (a few times, a millisecond apart) only while the open is refused the way a replace of the file refuses it.
pub fn read(path: &Path) -> Result<String, CredentialError> {
    read_with(|| std::fs::read_to_string(path), REFUSED_RETRIES, REFUSED_PAUSE)
}

fn read_with(
    mut open: impl FnMut() -> std::io::Result<String>,
    retries: u32,
    pause: std::time::Duration,
) -> Result<String, CredentialError> {
    let mut left = retries;
    loop {
        match open() {
            Ok(text) => {
                let text = text.trim();
                return if is_credential(text) { Ok(text.to_string()) } else { Err(CredentialError { failure: CredentialFailure::Invalid, os_code: None }) };
            }
            Err(error) if left > 0 && refused_by_a_replace(&error) => {
                left -= 1;
                std::thread::sleep(pause);
            }
            Err(error) => return Err(classify(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    const VALID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_refusal_that_ends_with_the_replace_reads_the_new_credential() {
        let mut refusals = 2;
        let read = read_with(
            || {
                if refusals > 0 {
                    refusals -= 1;
                    Err(Error::from(ErrorKind::PermissionDenied))
                } else {
                    Ok(format!("{VALID}\n"))
                }
            },
            REFUSED_RETRIES,
            std::time::Duration::ZERO,
        );
        assert_eq!(read.unwrap(), VALID);
        assert_eq!(refusals, 0);
    }

    #[test]
    fn a_refusal_that_does_not_end_is_named_as_denied_after_the_bounded_asks() {
        let mut asked = 0;
        let error = read_with(
            || {
                asked += 1;
                Err(Error::from(ErrorKind::PermissionDenied))
            },
            REFUSED_RETRIES,
            std::time::Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(error.failure, CredentialFailure::Denied);
        assert_eq!(asked, 1 + REFUSED_RETRIES);
    }

    #[test]
    fn a_missing_file_is_asked_for_once_and_named_as_missing() {
        let mut asked = 0;
        let error = read_with(
            || {
                asked += 1;
                Err(Error::from(ErrorKind::NotFound))
            },
            REFUSED_RETRIES,
            std::time::Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(error.failure, CredentialFailure::Missing);
        assert_eq!(asked, 1);
    }

    #[test]
    fn an_other_read_error_is_asked_for_once_and_named_as_unreadable() {
        let mut asked = 0;
        let error = read_with(
            || {
                asked += 1;
                Err(Error::from(ErrorKind::InvalidData))
            },
            REFUSED_RETRIES,
            std::time::Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(error.failure, CredentialFailure::Unreadable);
        assert_eq!(asked, 1);
    }

    #[test]
    fn a_value_that_is_not_a_credential_is_invalid_and_not_asked_for_again() {
        for text in ["", "not-a-token", &VALID[..63], &format!("{VALID}0"), &VALID.replace('a', "g")] {
            let mut asked = 0;
            let error = read_with(
                || {
                    asked += 1;
                    Ok(text.to_string())
                },
                REFUSED_RETRIES,
                std::time::Duration::ZERO,
            )
            .unwrap_err();
            assert_eq!(error.failure, CredentialFailure::Invalid, "{text:?}");
            assert_eq!(asked, 1);
        }
    }

    #[test]
    fn the_message_names_the_failure_and_the_os_error_and_nothing_of_the_file() {
        let error = CredentialError { failure: CredentialFailure::Denied, os_code: Some(5) };
        let text = error.to_string();
        assert_eq!(text, "Local host credential is unavailable (access denied) [os error 5]");
        assert!(!text.contains('/') && !text.contains('\\'));
        assert_eq!(CredentialFailure::Missing.label(), "credential-missing");
    }

    #[test]
    fn the_file_on_disk_is_read_and_a_missing_one_is_named() {
        let directory = std::env::temp_dir().join(format!("local-credential-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local-auth.token");
        assert_eq!(read(&path).unwrap_err().failure, CredentialFailure::Missing);
        std::fs::write(&path, format!("{VALID}\n")).unwrap();
        assert_eq!(read(&path).unwrap(), VALID);
        std::fs::write(&path, "not-a-token").unwrap();
        assert_eq!(read(&path).unwrap_err().failure, CredentialFailure::Invalid);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
