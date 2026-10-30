use anyhow::Result;
use std::path::{Path, PathBuf};
use tkless_core::TklessError;

fn resolved(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return path.canonicalize();
    }
    let absolute = std::path::absolute(path)?;
    let parent = absolute.parent().unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()?
        .join(absolute.file_name().unwrap_or_default()))
}

pub(crate) fn distinct(left: Option<&Path>, right: Option<&Path>) -> Result<()> {
    if let (Some(left), Some(right)) = (left, right) {
        if resolved(left)? == resolved(right)? {
            return Err(TklessError::InvalidArgument(
                "Input, output and checkpoint must be different files".into(),
            )
            .into());
        }
    }
    Ok(())
}
