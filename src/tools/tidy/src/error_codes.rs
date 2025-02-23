//! Tidy check to ensure error codes are properly documented and tested.
//!
//! Overview of check:
//!
//! 1. We create a list of error codes used by the compiler. Error codes are extracted from `compiler/rustc_error_codes/src/error_codes.rs`.
//!
//! 2. We check that the error code has a long-form explanation in `compiler/rustc_error_codes/src/error_codes/`.
//!   - The explanation is expected to contain a `doctest` that fails with the correct error code. (`EXEMPT_FROM_DOCTEST` *currently* bypasses this check)
//!   - Note that other stylistic conventions for markdown files are checked in the `style.rs` tidy check.
//!
//! 3. We check that the error code has a UI test in `tests/ui/error-codes/`.
//!   - We ensure that there is both a `Exxxx.rs` file and a corresponding `Exxxx.stderr` file.
//!   - We also ensure that the error code is used in the tests.
//!   - *Currently*, it is possible to opt-out of this check with the `EXEMPTED_FROM_TEST` constant.
//!
//! 4. We check that the error code is actually emitted by the compiler.
//!   - This is done by searching `compiler/` with a regex.

use std::{ffi::OsStr, fs, path::Path};

use regex::Regex;

use crate::walk::{filter_dirs, walk, walk_many};

const ERROR_CODES_PATH: &str = "compiler/rustc_error_codes/src/error_codes.rs";
const ERROR_DOCS_PATH: &str = "compiler/rustc_error_codes/src/error_codes/";
const ERROR_TESTS_PATH: &str = "tests/ui/error-codes/";

// Error codes that (for some reason) can't have a doctest in their explanation. Error codes are still expected to provide a code example, even if untested.
const IGNORE_DOCTEST_CHECK: &[&str] = &["E0464", "E0570", "E0601", "E0602", "E0640", "E0717"];

// Error codes that don't yet have a UI test. This list will eventually be removed.
const IGNORE_UI_TEST_CHECK: &[&str] =
    &["E0461", "E0465", "E0514", "E0554", "E0640", "E0717", "E0729"];

macro_rules! verbose_print {
    ($verbose:expr, $($fmt:tt)*) => {
        if $verbose {
            println!("{}", format_args!($($fmt)*));
        }
    };
}

pub fn check(root_path: &Path, search_paths: &[&Path], verbose: bool, bad: &mut bool) {
    let mut errors = Vec::new();

    // Stage 1: create list
    let error_codes = extract_error_codes(root_path, &mut errors);
    println!("Found {} error codes", error_codes.len());
    println!("Highest error code: `{}`", error_codes.iter().max().unwrap());

    // Stage 2: check list has docs
    let no_longer_emitted = check_error_codes_docs(root_path, &error_codes, &mut errors, verbose);

    // Stage 3: check list has UI tests
    check_error_codes_tests(root_path, &error_codes, &mut errors, verbose, &no_longer_emitted);

    // Stage 4: check list is emitted by compiler
    check_error_codes_used(search_paths, &error_codes, &mut errors, &no_longer_emitted, verbose);

    // Print any errors.
    for error in errors {
        tidy_error!(bad, "{}", error);
    }
}

/// Stage 1: Parses a list of error codes from `error_codes.rs`.
fn extract_error_codes(root_path: &Path, errors: &mut Vec<String>) -> Vec<String> {
    let path = root_path.join(Path::new(ERROR_CODES_PATH));
    let file =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read `{path:?}`: {e}"));

    let mut error_codes = Vec::new();

    for line in file.lines() {
        let line = line.trim();

        if line.starts_with('E') {
            let split_line = line.split_once(':');

            // Extract the error code from the line, emitting a fatal error if it is not in a correct format.
            let err_code = if let Some(err_code) = split_line {
                err_code.0.to_owned()
            } else {
                errors.push(format!(
                    "Expected a line with the format `Exxxx: include_str!(\"..\")`, but got \"{}\" \
                    without a `:` delimiter",
                    line,
                ));
                continue;
            };

            // If this is a duplicate of another error code, emit a fatal error.
            if error_codes.contains(&err_code) {
                errors.push(format!("Found duplicate error code: `{}`", err_code));
                continue;
            }

            // Ensure that the line references the correct markdown file.
            let expected_filename = format!(" include_str!(\"./error_codes/{}.md\"),", err_code);
            if expected_filename != split_line.unwrap().1 {
                errors.push(format!(
                    "Error code `{}` expected to reference docs with `{}` but instead found `{}` in \
                    `compiler/rustc_error_codes/src/error_codes.rs`",
                    err_code,
                    expected_filename,
                    split_line.unwrap().1,
                ));
                continue;
            }

            error_codes.push(err_code);
        }
    }

    error_codes
}

/// Stage 2: Checks that long-form error code explanations exist and have doctests.
fn check_error_codes_docs(
    root_path: &Path,
    error_codes: &[String],
    errors: &mut Vec<String>,
    verbose: bool,
) -> Vec<String> {
    let docs_path = root_path.join(Path::new(ERROR_DOCS_PATH));

    let mut no_longer_emitted_codes = Vec::new();

    walk(&docs_path, &mut |_| false, &mut |entry, contents| {
        let path = entry.path();

        // Error if the file isn't markdown.
        if path.extension() != Some(OsStr::new("md")) {
            errors.push(format!(
                "Found unexpected non-markdown file in error code docs directory: {}",
                path.display()
            ));
            return;
        }

        // Make sure that the file is referenced in `error_codes.rs`
        let filename = path.file_name().unwrap().to_str().unwrap().split_once('.');
        let err_code = filename.unwrap().0; // `unwrap` is ok because we know the filename is in the correct format.

        if error_codes.iter().all(|e| e != err_code) {
            errors.push(format!(
                "Found valid file `{}` in error code docs directory without corresponding \
                entry in `error_code.rs`",
                path.display()
            ));
            return;
        }

        let (found_code_example, found_proper_doctest, emit_ignore_warning, no_longer_emitted) =
            check_explanation_has_doctest(&contents, &err_code);

        if emit_ignore_warning {
            verbose_print!(
                verbose,
                "warning: Error code `{err_code}` uses the ignore header. This should not be used, add the error code to the \
                `IGNORE_DOCTEST_CHECK` constant instead."
            );
        }

        if no_longer_emitted {
            no_longer_emitted_codes.push(err_code.to_owned());
        }

        if !found_code_example {
            verbose_print!(
                verbose,
                "warning: Error code `{err_code}` doesn't have a code example, all error codes are expected to have one \
                (even if untested)."
            );
            return;
        }

        let test_ignored = IGNORE_DOCTEST_CHECK.contains(&&err_code);

        // Check that the explanation has a doctest, and if it shouldn't, that it doesn't
        if !found_proper_doctest && !test_ignored {
            errors.push(format!(
                "`{}` doesn't use its own error code in compile_fail example",
                path.display(),
            ));
        } else if found_proper_doctest && test_ignored {
            errors.push(format!(
                "`{}` has a compile_fail doctest with its own error code, it shouldn't \
                be listed in `IGNORE_DOCTEST_CHECK`",
                path.display(),
            ));
        }
    });

    no_longer_emitted_codes
}

/// This function returns a tuple indicating whether the provided explanation:
/// a) has a code example, tested or not.
/// b) has a valid doctest
fn check_explanation_has_doctest(explanation: &str, err_code: &str) -> (bool, bool, bool, bool) {
    let mut found_code_example = false;
    let mut found_proper_doctest = false;

    let mut emit_ignore_warning = false;
    let mut no_longer_emitted = false;

    for line in explanation.lines() {
        let line = line.trim();

        if line.starts_with("```") {
            found_code_example = true;

            // Check for the `rustdoc` doctest headers.
            if line.contains("compile_fail") && line.contains(err_code) {
                found_proper_doctest = true;
            }

            if line.contains("ignore") {
                emit_ignore_warning = true;
                found_proper_doctest = true;
            }
        } else if line
            .starts_with("#### Note: this error code is no longer emitted by the compiler")
        {
            no_longer_emitted = true;
            found_code_example = true;
            found_proper_doctest = true;
        }
    }

    (found_code_example, found_proper_doctest, emit_ignore_warning, no_longer_emitted)
}

// Stage 3: Checks that each error code has a UI test in the correct directory
fn check_error_codes_tests(
    root_path: &Path,
    error_codes: &[String],
    errors: &mut Vec<String>,
    verbose: bool,
    no_longer_emitted: &[String],
) {
    let tests_path = root_path.join(Path::new(ERROR_TESTS_PATH));

    for code in error_codes {
        let test_path = tests_path.join(format!("{}.stderr", code));

        if !test_path.exists() && !IGNORE_UI_TEST_CHECK.contains(&code.as_str()) {
            verbose_print!(
                verbose,
                "warning: Error code `{code}` needs to have at least one UI test in the `tests/error-codes/` directory`!"
            );
            continue;
        }
        if IGNORE_UI_TEST_CHECK.contains(&code.as_str()) {
            if test_path.exists() {
                errors.push(format!(
                    "Error code `{code}` has a UI test in `tests/ui/error-codes/{code}.rs`, it shouldn't be listed in `EXEMPTED_FROM_TEST`!"
                ));
            }
            continue;
        }

        let file = match fs::read_to_string(&test_path) {
            Ok(file) => file,
            Err(err) => {
                verbose_print!(
                    verbose,
                    "warning: Failed to read UI test file (`{}`) for `{code}` but the file exists. The test is assumed to work:\n{err}",
                    test_path.display()
                );
                continue;
            }
        };

        if no_longer_emitted.contains(code) {
            // UI tests *can't* contain error codes that are no longer emitted.
            continue;
        }

        let mut found_code = false;

        for line in file.lines() {
            let s = line.trim();
            // Assuming the line starts with `error[E`, we can substring the error code out.
            if s.starts_with("error[E") {
                if &s[6..11] == code {
                    found_code = true;
                    break;
                }
            };
        }

        if !found_code {
            verbose_print!(
                verbose,
                "warning: Error code {code}`` has a UI test file, but doesn't contain its own error code!"
            );
        }
    }
}

/// Stage 4: Search `compiler/` and ensure that every error code is actually used by the compiler and that no undocumented error codes exist.
fn check_error_codes_used(
    search_paths: &[&Path],
    error_codes: &[String],
    errors: &mut Vec<String>,
    no_longer_emitted: &[String],
    verbose: bool,
) {
    // We want error codes which match the following cases:
    //
    // * foo(a, E0111, a)
    // * foo(a, E0111)
    // * foo(E0111, a)
    // * #[error = "E0111"]
    let regex = Regex::new(r#"[(,"\s](E\d{4})[,)"]"#).unwrap();

    let mut found_codes = Vec::new();

    walk_many(search_paths, &mut filter_dirs, &mut |entry, contents| {
        let path = entry.path();

        // Return early if we aren't looking at a source file.
        if path.extension() != Some(OsStr::new("rs")) {
            return;
        }

        for line in contents.lines() {
            // We want to avoid parsing error codes in comments.
            if line.trim_start().starts_with("//") {
                continue;
            }

            for cap in regex.captures_iter(line) {
                if let Some(error_code) = cap.get(1) {
                    let error_code = error_code.as_str().to_owned();

                    if !error_codes.contains(&error_code) {
                        // This error code isn't properly defined, we must error.
                        errors.push(format!("Error code `{}` is used in the compiler but not defined and documented in `compiler/rustc_error_codes/src/error_codes.rs`.", error_code));
                        continue;
                    }

                    // This error code can now be marked as used.
                    found_codes.push(error_code);
                }
            }
        }
    });

    for code in error_codes {
        if !found_codes.contains(code) && !no_longer_emitted.contains(code) {
            errors.push(format!("Error code `{code}` exists, but is not emitted by the compiler!"))
        }

        if found_codes.contains(code) && no_longer_emitted.contains(code) {
            verbose_print!(
                verbose,
                "warning: Error code `{code}` is used when it's marked as \"no longer emitted\""
            );
        }
    }
}
