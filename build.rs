use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=assets/opencode-bridge");
    println!("cargo:rerun-if-changed=docs/terminfo");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    let root = env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory");
    let root = Path::new(&root);
    let revision =
        git(root, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    // Worktrees keep HEAD/index in their actual gitdir; watching .git alone
    // would leave revision metadata stale across commits/checkouts.
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(root, &["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(branch) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(root, &["rev-parse", "--git-path", &branch]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let (fingerprint, dirty) = source_fingerprint(root);

    println!("cargo:rustc-env=PANELINK_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=PANELINK_SOURCE_FINGERPRINT={fingerprint:016x}");
    println!("cargo:rustc-env=PANELINK_BUILD_DIRTY={dirty}");
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn source_fingerprint(root: &Path) -> (u64, bool) {
    let tracked = git(
        root,
        &[
            "ls-files",
            "-z",
            "--",
            "src",
            "vendor",
            "assets/opencode-bridge",
            "docs/terminfo",
            "build.rs",
            "Cargo.toml",
            "Cargo.lock",
        ],
    )
    .unwrap_or_default();
    let untracked = git(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "src",
            "vendor",
            "assets/opencode-bridge",
            "docs/terminfo",
            "build.rs",
            "Cargo.toml",
            "Cargo.lock",
        ],
    )
    .unwrap_or_default();
    let mut entries: Vec<String> = tracked
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect();
    if entries.is_empty() {
        for name in [
            "src",
            "vendor",
            "assets/opencode-bridge",
            "docs/terminfo",
            "build.rs",
            "Cargo.toml",
            "Cargo.lock",
        ] {
            collect_files(root, &root.join(name), &mut entries);
        }
    }
    entries.sort_unstable();
    entries.dedup();

    let mut hash = 0xcbf29ce484222325_u64;
    let mut dirty = !untracked.is_empty();
    for entry in entries {
        let path = root.join(&entry);
        println!("cargo:rerun-if-changed={entry}");
        hash_bytes(&mut hash, entry.as_bytes());
        hash_bytes(&mut hash, &[0]);
        match fs::read(path) {
            Ok(bytes) => {
                hash_bytes(&mut hash, &[1]);
                hash_bytes(&mut hash, &bytes);
            }
            Err(_) => {
                hash_bytes(&mut hash, &[0]);
                dirty = true;
            }
        }
        hash_bytes(&mut hash, &[0xff]);
    }
    dirty |= Command::new("git")
        .args([
            "diff",
            "--quiet",
            "HEAD",
            "--",
            "src",
            "vendor",
            "assets/opencode-bridge",
            "docs/terminfo",
            "build.rs",
            "Cargo.toml",
            "Cargo.lock",
        ])
        .current_dir(root)
        .status()
        .map_or(true, |status| !status.success());
    (hash, dirty)
}

fn collect_files(root: &Path, path: &Path, entries: &mut Vec<String>) {
    if path.is_dir() {
        let mut children = fs::read_dir(path)
            .expect("build input directory")
            .map(|item| item.expect("build input entry").path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            collect_files(root, &child, entries);
        }
    } else if path.is_file() {
        entries.push(
            path.strip_prefix(root)
                .expect("relative build input")
                .to_string_lossy()
                .into_owned(),
        );
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}
