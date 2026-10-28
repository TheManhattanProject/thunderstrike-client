//! Finds the prebuilt `thunderstrike_client` library and links it.
//!
//! In order:
//! 1. `DOCS_RS` set: link nothing (docs.rs has no network, and rustdoc
//!    never links).
//! 2. `THUNDERSTRIKE_LIB_DIR` set: link from that directory. For offline
//!    and air-gapped builds; nothing is downloaded or verified.
//! 3. Otherwise download `thunderstrike-native-{version}-{rid}.tar.gz` from
//!    the release (or `THUNDERSTRIKE_DOWNLOAD_BASE`, for mirrors), check it
//!    against `checksums.txt`, extract it into `OUT_DIR` and link that.
//!
//! The download shells out to `curl` and `tar`, which ship with Windows 10+,
//! macOS and every Linux distribution. SHA-256 is implemented below rather
//! than pulled in as a build-dependency: thirty lines keep the crate at zero
//! dependencies, and the code that decides whether a binary is trusted is
//! then all in this one file.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIB: &str = "thunderstrike_client";
const RELEASES: &str = "https://github.com/TheManhattanProject/thunderstrike-client/releases/download";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=checksums.txt");
    for v in [
        "DOCS_RS",
        "THUNDERSTRIKE_LIB_DIR",
        "THUNDERSTRIKE_DOWNLOAD_BASE",
        "THUNDERSTRIKE_SKIP_VERIFY",
    ] {
        println!("cargo:rerun-if-env-changed={v}");
    }

    if env::var_os("DOCS_RS").is_some() {
        return;
    }
    if let Err(msg) = run() {
        eprintln!("\nthunderstrike: {msg}\n");
        // A panic would bury the message under a backtrace hint.
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|_| "TARGET is not set".to_string())?;
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);

    let lib_dir = match env::var_os("THUNDERSTRIKE_LIB_DIR") {
        Some(d) => {
            let d = std::path::absolute(PathBuf::from(d))
                .map_err(|e| format!("THUNDERSTRIKE_LIB_DIR: {e}"))?;
            if !d.is_dir() {
                return Err(format!("THUNDERSTRIKE_LIB_DIR={} is not a directory", d.display()));
            }
            d
        }
        None => fetch(&target, &out)?,
    };

    link(&target, &lib_dir, &out)
}

/// The bundle's runtime id for a Rust target (docs/PUBLIC_RELEASE.md).
fn rid(target: &str) -> Option<&'static str> {
    Some(match target {
        "x86_64-unknown-linux-gnu" => "linux-x64",
        "aarch64-unknown-linux-gnu" => "linux-arm64",
        "x86_64-unknown-linux-musl" => "linux-x64-musl",
        "aarch64-unknown-linux-musl" => "linux-arm64-musl",
        "x86_64-apple-darwin" => "osx-x64",
        "aarch64-apple-darwin" => "osx-arm64",
        "x86_64-pc-windows-msvc" => "win-x64",
        "x86_64-pc-windows-gnu" => "win-x64-gnu",
        _ => return None,
    })
}

/// Download, verify and extract the bundle; returns its `<rid>/` directory.
fn fetch(target: &str, out: &Path) -> Result<PathBuf, String> {
    let rid = rid(target).ok_or_else(|| {
        format!(
            "no prebuilt library for target {target}. Supported: x86_64/aarch64 Linux \
             (gnu, musl), x86_64/aarch64 macOS, x86_64 Windows (msvc, gnu). \
             Set THUNDERSTRIKE_LIB_DIR to link a library you already have."
        )
    })?;
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let asset = format!("thunderstrike-native-{version}-{rid}.tar.gz");
    let base = env::var("THUNDERSTRIKE_DOWNLOAD_BASE")
        .unwrap_or_else(|_| format!("{RELEASES}/v{version}"));
    let url = format!("{}/{asset}", base.trim_end_matches('/'));

    let skip_verify = env::var("THUNDERSTRIKE_SKIP_VERIFY").is_ok_and(|v| v == "1");
    let pinned = pinned_sha256(&asset)?;
    if pinned.is_none() && !skip_verify {
        return Err(format!(
            "checksums.txt has no entry for {asset}, so the download cannot be verified. \
             This crate version was not released for {rid}. Set THUNDERSTRIKE_LIB_DIR to \
             link a local library, or THUNDERSTRIKE_SKIP_VERIFY=1 to trust the download \
             unchecked (not for production)."
        ));
    }

    let archive = out.join(&asset);
    let root = out.join("native");
    let lib_dir = root.join(rid);

    // A verified archive from an earlier run is not fetched again.
    let have = archive.is_file()
        && pinned
            .as_deref()
            .is_some_and(|p| sha256_file(&archive).is_ok_and(|h| h == p));
    if !have {
        let tmp = out.join(format!("{asset}.part"));
        let _ = fs::remove_file(&tmp);
        let status = Command::new(tool("curl"))
            .args(["--fail", "--location", "--silent", "--show-error"])
            .args(["--retry", "3", "--connect-timeout", "30"])
            .arg("--output")
            .arg(&tmp)
            .arg(&url)
            .status()
            .map_err(|e| format!("could not run curl ({e}); install curl or set THUNDERSTRIKE_LIB_DIR"))?;
        if !status.success() {
            return Err(format!("download failed: {url} (curl {status})"));
        }
        let got = sha256_file(&tmp)?;
        match &pinned {
            Some(want) if *want != got => {
                let _ = fs::remove_file(&tmp);
                return Err(format!(
                    "sha256 mismatch for {asset} from {url}:\n  expected {want}\n  got      {got}\n\
                     Refusing to link it."
                ));
            }
            Some(_) => {}
            None => println!("cargo:warning=thunderstrike: {asset} NOT verified (THUNDERSTRIKE_SKIP_VERIFY=1), sha256 {got}"),
        }
        fs::rename(&tmp, &archive).map_err(|e| format!("{}: {e}", archive.display()))?;
        let _ = fs::remove_dir_all(&root);
    }

    if !lib_dir.is_dir() {
        fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        let status = Command::new(tool("tar"))
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&root)
            .status()
            .map_err(|e| format!("could not run tar ({e})"))?;
        if !status.success() || !lib_dir.is_dir() {
            return Err(format!(
                "extracting {} did not produce {rid}/ (tar {status})",
                archive.display()
            ));
        }
    }
    Ok(lib_dir)
}

/// On Windows the system's own curl.exe/tar.exe, not whatever an MSYS or Git
/// shell puts first on PATH: GNU tar there reads `C:\...` as a remote host.
fn tool(name: &str) -> PathBuf {
    if cfg!(windows) {
        if let Some(root) = env::var_os("SystemRoot") {
            let p = Path::new(&root).join("System32").join(format!("{name}.exe"));
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(name)
}

/// The pinned hash for `asset`, from `checksums.txt` (`sha256sum` format).
fn pinned_sha256(asset: &str) -> Result<Option<String>, String> {
    let path = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("checksums.txt");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(hash), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        // sha256sum marks binary mode with a leading '*'.
        if name.trim_start_matches('*') == asset {
            let hash = hash.to_ascii_lowercase();
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("checksums.txt: malformed hash for {asset}"));
            }
            return Ok(Some(hash));
        }
    }
    Ok(None)
}

fn link(target: &str, lib_dir: &Path, out: &Path) -> Result<(), String> {
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // Dependents can find the header and library through DEP_THUNDERSTRIKE_CLIENT_*.
    println!("cargo:root={}", lib_dir.display());

    if target.ends_with("windows-msvc") {
        // No static archive for MSVC: link the DLL through its import
        // library. rustc appends ".lib", so this names thunderstrike_client.dll.lib.
        let dll = lib_dir.join(format!("{LIB}.dll"));
        if !lib_dir.join(format!("{LIB}.dll.lib")).is_file() || !dll.is_file() {
            return Err(format!(
                "{} must hold {LIB}.dll and {LIB}.dll.lib",
                lib_dir.display()
            ));
        }
        println!("cargo:rustc-link-lib=dylib={LIB}.dll");
        copy_dll_next_to_binaries(&dll, out)?;
        return Ok(());
    }

    if !lib_dir.join(format!("lib{LIB}.a")).is_file() {
        return Err(format!("{} has no lib{LIB}.a", lib_dir.display()));
    }
    println!("cargo:rustc-link-lib=static={LIB}");
    let system: &[&str] = if target.contains("apple-darwin") {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Security");
        &["m"]
    } else if target.ends_with("linux-musl") {
        &["m"]
    } else if target.contains("linux") {
        &["m", "dl", "pthread"]
    } else if target.ends_with("windows-gnu") {
        &["ws2_32", "userenv", "bcrypt", "ntdll", "advapi32"]
    } else {
        &[]
    };
    for l in system {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
    Ok(())
}

/// Windows finds a DLL next to the exe. OUT_DIR is
/// `target/[<triple>/]<profile>/build/<pkg>-<hash>/out`; binaries land in
/// `<profile>/`, test and bench binaries in `deps/`, examples in `examples/`.
fn copy_dll_next_to_binaries(dll: &Path, out: &Path) -> Result<(), String> {
    let Some(profile) = out.ancestors().nth(3) else {
        return Ok(());
    };
    for dir in [profile.to_path_buf(), profile.join("deps"), profile.join("examples")] {
        if fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let to = dir.join(dll.file_name().unwrap());
        // A running binary holds the old copy open; that is not this build's problem.
        if let Err(e) = fs::copy(dll, &to) {
            if !to.is_file() {
                return Err(format!("copying {} to {}: {e}", dll.display(), to.display()));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SHA-256 (FIPS 180-4).
// ---------------------------------------------------------------------------

fn sha256_file(path: &Path) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(sha256(&data).iter().map(|b| format!("{b:02x}")).collect())
}

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (o, x) in out.chunks_exact_mut(4).zip(h) {
        o.copy_from_slice(&x.to_be_bytes());
    }
    out
}
