//! Find a user requested python version/interpreter.

use std::path::PathBuf;
use std::process::Command;

use once_cell::sync::Lazy;
use regex::Regex;
use tracing::info_span;

use crate::{Error, Interpreter};

/// ```text
/// -V:3.12          C:\Users\Ferris\AppData\Local\Programs\Python\Python312\python.exe
/// -V:3.8           C:\Users\Ferris\AppData\Local\Programs\Python\Python38\python.exe
/// ```
static PY_LIST_PATHS: Lazy<Regex> = Lazy::new(|| {
    // Without the `R` flag, paths have trailing \r
    Regex::new(r"(?mR)^ -(?:V:)?(\d).(\d+)-?(?:arm)?(?:\d*)\s*\*?\s*(.*)$").unwrap()
});

/// Find a user requested python version/interpreter.
///
/// Supported formats:
/// * `-p 3.10` searches for an installed Python 3.10 (`py --list-paths` on Windows, `python3.10` on Linux/Mac).
///   Specifying a patch version is not supported.
/// * `-p python3.10` or `-p python.exe` looks for a binary in `PATH`.
/// * `-p /home/ferris/.local/bin/python3.10` uses this exact Python.
pub fn find_requested_python(request: &str) -> Result<PathBuf, Error> {
    let versions = request
        .splitn(3, '.')
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>();
    if let Ok(versions) = versions {
        // `-p 3.10` or `-p 3.10.1`
        if cfg!(unix) {
            let formatted = PathBuf::from(format!("python{request}"));
            Interpreter::find_executable(&formatted)
        } else if cfg!(windows) {
            if let [major, minor] = versions.as_slice() {
                find_python_windows(*major, *minor)?.ok_or(Error::NoSuchPython {
                    major: *major,
                    minor: *minor,
                })
            } else {
                unimplemented!("Patch versions cannot be requested on Windows")
            }
        } else {
            unimplemented!("Only Windows and Unix are supported")
        }
    } else if !request.contains(std::path::MAIN_SEPARATOR) {
        // `-p python3.10`; Generally not used on windows because all Python are `python.exe`.
        Interpreter::find_executable(request)
    } else {
        // `-p /home/ferris/.local/bin/python3.10`
        Ok(fs_err::canonicalize(request)?)
    }
}

/// Run `py --list-paths` to find the installed pythons.
///
/// The command takes 8ms on my machine. TODO(konstin): Implement <https://peps.python.org/pep-0514/> to read python
/// installations from the registry instead.
fn installed_pythons_windows() -> Result<Vec<(u8, u8, PathBuf)>, Error> {
    let output = info_span!("py_list_paths")
        .in_scope(|| Command::new("py").arg("--list-paths").output())
        .map_err(Error::PyList)?;

    // There shouldn't be any output on stderr.
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(Error::PythonSubcommandOutput {
            message: format!(
                "Running `py --list-paths` failed with status {}",
                output.status
            ),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    // Find the first python of the version we want in the list
    let stdout =
        String::from_utf8(output.stdout.clone()).map_err(|err| Error::PythonSubcommandOutput {
            message: format!("Running `py --list-paths` stdout isn't UTF-8 encoded: {err}"),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })?;
    Ok(PY_LIST_PATHS
        .captures_iter(&stdout)
        .filter_map(|captures| {
            let (_, [major, minor, path]) = captures.extract();
            Some((
                major.parse::<u8>().ok()?,
                minor.parse::<u8>().ok()?,
                PathBuf::from(path),
            ))
        })
        .collect())
}

pub(crate) fn find_python_windows(major: u8, minor: u8) -> Result<Option<PathBuf>, Error> {
    Ok(installed_pythons_windows()?
        .into_iter()
        .find(|(major_, minor_, _path)| *major_ == major && *minor_ == minor)
        .map(|(_, _, path)| path))
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use insta::{assert_display_snapshot, assert_snapshot};
    use itertools::Itertools;

    use crate::python_query::find_requested_python;
    use crate::Error;

    fn format_err<T: Debug>(err: Result<T, Error>) -> String {
        anyhow::Error::new(err.unwrap_err())
            .chain()
            .join("\n  Caused by: ")
    }

    #[cfg(unix)]
    #[test]
    fn python312() {
        assert_eq!(
            find_requested_python("3.12").unwrap(),
            find_requested_python("python3.12").unwrap()
        );
    }

    #[test]
    fn no_such_python_version() {
        assert_snapshot!(format_err(find_requested_python("3.1000")), @r###"
        Couldn't find "3.1000" in PATH
          Caused by: cannot find binary path
        "###);
    }

    #[test]
    fn no_such_python_binary() {
        assert_display_snapshot!(format_err(find_requested_python("python3.1000")), @r###"
        Couldn't find "python3.1000" in PATH
          Caused by: cannot find binary path
        "###);
    }

    #[test]
    fn no_such_python_path() {
        assert_display_snapshot!(
            format_err(find_requested_python("/does/not/exists/python3.12")), @r###"
        failed to canonicalize path `/does/not/exists/python3.12`
          Caused by: No such file or directory (os error 2)
        "###);
    }
}
