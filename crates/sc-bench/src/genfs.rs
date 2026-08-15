//! Synthetic directory-tree generators used by benchmarks and manual
//! stress-testing (100k / 1M entry directories).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Create `count` small files in `dir` (flat). Returns the directory path.
pub fn generate_flat(dir: &Path, count: usize) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for i in 0..count {
        let path = dir.join(format!("file_{i:07}.txt"));
        if !path.exists() {
            let mut f = std::fs::File::create(&path)?;
            f.write_all(b"x")?;
        }
    }
    Ok(dir.to_path_buf())
}

/// Create a tree: `breadth` dirs per level, `depth` levels, `files` per dir.
pub fn generate_tree(
    root: &Path,
    breadth: usize,
    depth: usize,
    files: usize,
) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    for f in 0..files {
        let p = root.join(format!("f{f:04}.dat"));
        if !p.exists() {
            std::fs::write(&p, b"data")?;
        }
    }
    if depth == 0 {
        return Ok(());
    }
    for b in 0..breadth {
        generate_tree(&root.join(format!("d{b:02}")), breadth, depth - 1, files)?;
    }
    Ok(())
}
