use std::path::PathBuf;
use xtask::project_root;

use crate::results::emit_compare;

// this is the filename of the results coming from `main` branch
const BASE_RESULT_FILE: &str = "base_results.json";

// this is the filename of the results coming from the current PR
const NEW_RESULT_FILE: &str = "new_results.json";

pub fn coverage_compare(
    base_result_path: Option<&str>,
    new_result_path: Option<&str>,
    markdown: bool,
) {
    // resolve the path passed as argument, or retrieve the default one
    let base_result_dir = if let Some(base_result_path) = base_result_path {
        PathBuf::from(base_result_path)
    } else {
        project_root().join(BASE_RESULT_FILE)
    };

    // resolve the path passed as argument, or retrieve the default one
    let new_result_dir = if let Some(new_result_path) = new_result_path {
        PathBuf::from(new_result_path)
    } else {
        project_root().join(NEW_RESULT_FILE)
    };

    if !base_result_dir.exists() {
        panic!(
            "The path to the base results doesn't exist: {:?}",
            base_result_dir
        );
    }

    if !&new_result_dir.exists() {
        panic!(
            "The path to the new results doesn't exist: {:?}",
            new_result_dir
        );
    }

    emit_compare(
        base_result_dir.as_path(),
        new_result_dir.as_path(),
        markdown,
    );
}