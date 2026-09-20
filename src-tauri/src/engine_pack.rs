//! 随安装包带来的建模引擎包(`cad-engine.tar.zst`)→ 解到应用数据目录。
//!
//! 「单个安装包、用户零安装」:引擎(内嵌 CPython + build123d,解开约 600 MB)打在安装包里,
//! 是**一个**压缩文件而不是上万个散文件——安装快;macOS 上 Python 的二进制也不用待在签名的 .app 里。
//! 第一次用到代码建模时由应用自己解开,用户什么都不用装、也不用联网。
//!
//! 步骤:校验 SHA-256(清单随包一起带着)→ 解到 `cad-engine/.incoming-<id>/` → 原子换名成 `cad-engine/python/`。
//! 中途断电 / 被杀只会留下一个 `.incoming-*` 目录,下次安装时清掉;已装好的引擎不会被半个新版本覆盖。

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ARCHIVE_NAME: &str = "cad-engine.tar.zst";
pub const MANIFEST_NAME: &str = "cad-engine.json";

/// 引擎包的清单(`scripts/build-engine-pack.py` 生成,和压缩包放在一起)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    pub engine_version: String,
    #[serde(default)]
    pub platform: String,
    pub sha256: String,
    pub bytes: u64,
    #[serde(default)]
    pub unpacked_bytes: u64,
    #[serde(default)]
    pub files: u64,
}

/// 已经解开的引擎里的 `python/engine.json`。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct InstalledMarker {
    #[serde(default)]
    pub engine_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Verifying,
    Extracting,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug)]
pub enum PackError {
    Io(String),
    /// 压缩包和清单对不上(下载 / 拷贝坏了,或者被人动过)
    Checksum { expected: String, actual: String },
    /// 包里有不该有的东西(路径想跑到目标目录外面去)
    Unsafe(String),
    Format(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::Io(e) => write!(f, "{e}"),
            PackError::Checksum { expected, actual } => write!(f, "engine pack checksum mismatch: expected {expected}, got {actual}"),
            PackError::Unsafe(p) => write!(f, "engine pack contains an unsafe path: {p}"),
            PackError::Format(e) => write!(f, "engine pack is not readable: {e}"),
        }
    }
}

fn io(e: std::io::Error) -> PackError {
    PackError::Io(e.to_string())
}

/// 安装包里带没带引擎包。开发构建里只有一个占位的 README,返回 `None`。
pub fn bundled(resource_dir: &Path) -> Option<(PathBuf, PackManifest)> {
    let dir = resource_dir.join("cad-engine");
    let archive = dir.join(ARCHIVE_NAME);
    let manifest: PackManifest = serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST_NAME)).ok()?).ok()?;
    archive.is_file().then_some((archive, manifest))
}

/// 已经解开的引擎是哪个版本;没装或读不出来返回 `None`。
pub fn installed_version(engine_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(engine_root.join("python").join("engine.json")).ok()?;
    let marker: InstalledMarker = serde_json::from_str(&text).ok()?;
    (!marker.engine_version.is_empty()).then_some(marker.engine_version)
}

/// 读的同时数字节数(进度)。
struct Counting<R, F: FnMut(u64)> {
    inner: R,
    seen: u64,
    every: u64,
    next_report: u64,
    report: F,
}

impl<R: Read, F: FnMut(u64)> Read for Counting<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.seen += n as u64;
        if self.seen >= self.next_report || n == 0 {
            self.next_report = self.seen + self.every;
            (self.report)(self.seen);
        }
        Ok(n)
    }
}

fn counting<R: Read, F: FnMut(u64)>(inner: R, total: u64, report: F) -> Counting<R, F> {
    let every = (total / 200).max(64 * 1024); // 最多报 200 次
    Counting {
        inner,
        seen: 0,
        every,
        next_report: every,
        report,
    }
}

pub fn sha256_file(path: &Path, mut on_bytes: impl FnMut(u64)) -> Result<String, PackError> {
    let file = File::open(path).map_err(io)?;
    let total = file.metadata().map_err(io)?.len();
    let mut reader = counting(BufReader::with_capacity(1 << 20, file), total, &mut on_bytes);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// tar 里的路径只能落在 `python/` 下面:相对路径、没有 `..`、没有盘符。
fn safe_entry_path(path: &Path) -> bool {
    let mut parts = path.components();
    matches!(parts.next(), Some(Component::Normal(first)) if first == "python") && parts.all(|c| matches!(c, Component::Normal(_)))
}

/// 把 tar 流解到 `dest`(调用方给的空目录)。符号链接只允许指向包内的相对位置。
pub fn unpack_tar(reader: impl Read, dest: &Path) -> Result<u64, PackError> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(true);
    archive.set_preserve_mtime(false);
    archive.set_overwrite(false);
    let mut files = 0;
    for entry in archive.entries().map_err(|e| PackError::Format(e.to_string()))? {
        let mut entry = entry.map_err(|e| PackError::Format(e.to_string()))?;
        let path = entry.path().map_err(|e| PackError::Format(e.to_string()))?.into_owned();
        if !safe_entry_path(&path) {
            return Err(PackError::Unsafe(path.display().to_string()));
        }
        if let Some(target) = entry.link_name().map_err(|e| PackError::Format(e.to_string()))? {
            let escapes = target.is_absolute() || target.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir));
            // `..` 在包内的相对链接里是正常的(bin/python3 → python3.12 之类不需要,但 lib 里有);
            // 拼起来之后仍然得落在 python/ 里面
            let resolved = path.parent().unwrap_or(Path::new("")).join(&target);
            let mut depth: i32 = 0;
            for c in resolved.components() {
                match c {
                    Component::Normal(_) => depth += 1,
                    Component::ParentDir => depth -= 1,
                    _ => {}
                }
                if depth < 1 {
                    return Err(PackError::Unsafe(format!("{} -> {}", path.display(), target.display())));
                }
            }
            if escapes {
                return Err(PackError::Unsafe(format!("{} -> {}", path.display(), target.display())));
            }
        }
        // unpack_in 自己也会拒绝跑出 dest 的路径——两道保险
        if !entry.unpack_in(dest).map_err(io)? {
            return Err(PackError::Unsafe(path.display().to_string()));
        }
        files += 1;
    }
    Ok(files)
}

fn remove_dir_with_retry(path: &Path) -> std::io::Result<()> {
    for attempt in 0..10 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt == 9 => return Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    Ok(())
}

/// 清掉上次没装完留下的 `.incoming-*` / `.old-*`。
pub fn clean_leftovers(engine_root: &Path) {
    let Ok(entries) = std::fs::read_dir(engine_root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".incoming-") || name.starts_with(".old-") {
            let _ = remove_dir_with_retry(&entry.path());
        }
    }
}

/// 校验并安装。`engine_root` 是 `<应用数据>/cad-engine`;装好之后解释器在 `engine_root/python/` 下。
/// 调用方要保证这时没有引擎进程在跑(Windows 上正在运行的 python.exe 删不掉)。
pub fn install(
    archive: &Path,
    manifest: &PackManifest,
    engine_root: &Path,
    tag: &str,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<u64, PackError> {
    std::fs::create_dir_all(engine_root).map_err(io)?;
    clean_leftovers(engine_root);

    let total = std::fs::metadata(archive).map_err(io)?.len();
    let actual = sha256_file(archive, |done| {
        on_progress(Progress {
            phase: Phase::Verifying,
            done,
            total,
        })
    })?;
    if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
        return Err(PackError::Checksum {
            expected: manifest.sha256.clone(),
            actual,
        });
    }

    let incoming = engine_root.join(format!(".incoming-{tag}"));
    std::fs::create_dir_all(&incoming).map_err(io)?;
    let file = File::open(archive).map_err(io)?;
    let compressed = counting(BufReader::with_capacity(1 << 20, file), total, |done| {
        on_progress(Progress {
            phase: Phase::Extracting,
            done,
            total,
        })
    });
    let decoder = ruzstd::decoding::StreamingDecoder::new(compressed).map_err(|e| PackError::Format(e.to_string()))?;
    let files = match unpack_tar(decoder, &incoming) {
        Ok(n) => n,
        Err(e) => {
            let _ = remove_dir_with_retry(&incoming);
            return Err(e);
        }
    };

    // 换上去:旧的先挪开(挪不动说明有进程占着,这时不能硬删),新的换名到位,再删旧的
    let live = engine_root.join("python");
    let old = engine_root.join(format!(".old-{tag}"));
    if live.exists() {
        std::fs::rename(&live, &old).map_err(io)?;
    }
    if let Err(e) = std::fs::rename(incoming.join("python"), &live) {
        if old.exists() {
            let _ = std::fs::rename(&old, &live); // 回滚
        }
        let _ = remove_dir_with_retry(&incoming);
        return Err(io(e));
    }
    let _ = remove_dir_with_retry(&incoming);
    let _ = remove_dir_with_retry(&old);
    on_progress(Progress {
        phase: Phase::Done,
        done: total,
        total,
    });
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pp-engine-pack-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 造一个小的 tar(不压缩)。
    fn tar_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn a_well_formed_pack_unpacks_under_python() {
        let dest = scratch("ok");
        let tar = tar_of(&[("python/engine.json", br#"{"engine_version":"t-1"}"#), ("python/Lib/site.py", b"x = 1\n")]);
        assert_eq!(unpack_tar(&tar[..], &dest).unwrap(), 2);
        assert_eq!(std::fs::read(dest.join("python").join("Lib").join("site.py")).unwrap(), b"x = 1\n");
        assert_eq!(installed_version(&dest).as_deref(), Some("t-1"));
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn entries_outside_python_are_refused() {
        for bad in ["evil.txt", "other/x.py", "python/../../escape.txt"] {
            assert!(!safe_entry_path(Path::new(bad)), "{bad}");
        }
        assert!(safe_entry_path(Path::new("python/bin/python3")));

        let dest = scratch("evil");
        let tar = tar_of(&[("python/ok.txt", b"ok"), ("outside.txt", b"nope")]);
        assert!(matches!(unpack_tar(&tar[..], &dest), Err(PackError::Unsafe(p)) if p.contains("outside.txt")));
        assert!(!dest.join("outside.txt").exists());
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn checksums_are_hex_sha256_of_the_whole_file() {
        let dir = scratch("sha");
        let file = dir.join("a.bin");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_file(&file, |_| {}).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pack_that_does_not_match_its_manifest_is_never_unpacked() {
        let dir = scratch("mismatch");
        let archive = dir.join(ARCHIVE_NAME);
        std::fs::write(&archive, b"not the real thing").unwrap();
        let manifest = PackManifest {
            engine_version: "t-1".into(),
            platform: String::new(),
            sha256: "00".repeat(32),
            bytes: 18,
            unpacked_bytes: 0,
            files: 0,
        };
        let root = dir.join("cad-engine");
        let seen = Mutex::new(Vec::new());
        let err = install(&archive, &manifest, &root, "x", &|p| seen.lock().unwrap().push(p.phase)).unwrap_err();
        assert!(matches!(err, PackError::Checksum { .. }), "{err}");
        assert!(!root.join("python").exists());
        assert!(seen.lock().unwrap().iter().all(|p| *p == Phase::Verifying), "校验没过就不该开始解包");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dev_build_without_a_pack_reports_nothing_bundled() {
        let dir = scratch("none");
        std::fs::create_dir_all(dir.join("cad-engine")).unwrap();
        std::fs::write(dir.join("cad-engine").join("README.txt"), "placeholder").unwrap();
        assert!(bundled(&dir).is_none());
        assert_eq!(installed_version(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 本机如果已经跑过 `scripts/build-engine-pack.py`,就拿真的引擎包走一遍完整安装(约一分钟,所以默认不跑)。
    #[test]
    #[ignore = "needs src-tauri/resources/cad-engine/cad-engine.tar.zst; run with --ignored"]
    fn the_real_pack_installs_and_the_engine_inside_it_starts() {
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let (archive, manifest) = bundled(&resources).expect("run scripts/build-engine-pack.py first");
        let dir = scratch("real");
        let root = dir.join("cad-engine");
        let started = std::time::Instant::now();
        let files = install(&archive, &manifest, &root, "t", &|_| {}).expect("install");
        eprintln!("unpacked {files} files in {:?}", started.elapsed());
        assert_eq!(files, manifest.files);
        assert_eq!(installed_version(&root).as_deref(), Some(manifest.engine_version.as_str()));

        let engine = pp_cad::Engine::locate(&dir).expect("the unpacked interpreter is where Engine::locate looks");
        let info = engine.probe().expect("the unpacked engine starts");
        assert_eq!(info.build123d_version, "0.12.0");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
