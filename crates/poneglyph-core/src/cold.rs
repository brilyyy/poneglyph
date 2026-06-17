//! Cold storage — zstd-compressed memory files for the cold tier.
//!
//! When a memory is moved to cold tier, its content is compressed to a `.zst`
//! file under `~/.local/share/poneglyph/cold/{project_id}/{memory_id}.zst`.
//! The memory row keeps a `cold_path` pointer for lazy decompression.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::info;

/// Base directory for cold storage files.
pub fn cold_dir() -> PathBuf {
    crate::config::Config::data_dir().join("cold")
}

/// Compress content to a `.zst` file. Returns the path.
pub fn compress_to_file(
    content: &str,
    project_id: &str,
    memory_id: &str,
    level: i32,
) -> Result<PathBuf> {
    let dir = cold_dir().join(project_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create cold dir: {}", dir.display()))?;

    let path = dir.join(format!("{memory_id}.zst"));
    let mut encoder = zstd::Encoder::new(Vec::new(), level)
        .context("failed to create zstd encoder")?;
    encoder.write_all(content.as_bytes())
        .context("failed to write content to encoder")?;
    let compressed = encoder.finish()
        .context("failed to finish zstd encoding")?;

    std::fs::write(&path, &compressed)
        .with_context(|| format!("failed to write cold file: {}", path.display()))?;

    info!(
        memory_id,
        project_id,
        original_bytes = content.len(),
        compressed_bytes = compressed.len(),
        path = %path.display(),
        "memory moved to cold storage"
    );

    Ok(path)
}

/// Decompress content from a `.zst` file.
pub fn decompress_from_file(path: &Path) -> Result<String> {
    let compressed = std::fs::read(path)
        .with_context(|| format!("failed to read cold file: {}", path.display()))?;

    let mut decoder = zstd::Decoder::new(&compressed[..])
        .context("failed to create zstd decoder")?;
    let mut content = String::new();
    decoder.read_to_string(&mut content)
        .context("failed to decode zstd content")?;

    Ok(content)
}

/// Delete a cold file if it exists.
pub fn delete_cold_file(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to delete cold file: {}", path.display()))?;
    }
    Ok(())
}

/// Get the cold path for a memory.
pub fn cold_path_for(project_id: &str, memory_id: &str) -> PathBuf {
    cold_dir().join(project_id).join(format!("{memory_id}.zst"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_and_decompress() {
        let tmp = tempfile::tempdir().unwrap();
        let original = "Hello, this is a test memory content that should compress well. \
                        It has enough repetition to demonstrate compression benefits. \
                        Cold storage uses zstd for efficient compression of text memories.";

        let path = compress_to_file(original, "test-project", "test-mem", 3).unwrap();
        assert!(path.exists());

        let decompressed = decompress_from_file(&path).unwrap();
        assert_eq!(original, decompressed);

        delete_cold_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn cold_path_for_format() {
        let path = cold_path_for("proj-123", "mem-456");
        assert!(path.to_string_lossy().contains("proj-123"));
        assert!(path.to_string_lossy().contains("mem-456"));
        assert!(path.to_string_lossy().ends_with(".zst"));
    }
}
