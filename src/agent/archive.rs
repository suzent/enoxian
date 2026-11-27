//! Download, verify and unpack a prebuilt agent release.
//!
//! Used by the plugin installer for ACP Registry entries distributed as
//! per-platform archives. The format is sniffed from the file's leading bytes,
//! not from the URL, because some publishers serve archives from URLs without
//! an extension and others serve a bare executable.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Ceiling on one download. The largest registry releases (whole app bundles)
/// are a few hundred megabytes.
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    TarGz,
    TarBz2,
    TarXz,
    Tar,
    Zip,
    /// Not an archive: the download is the executable itself.
    Raw,
}

pub fn sniff(head: &[u8]) -> Format {
    if head.starts_with(&[0x1f, 0x8b]) {
        Format::TarGz
    } else if head.starts_with(b"BZh") {
        Format::TarBz2
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Format::TarXz
    } else if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        Format::Zip
    } else if head.len() > 262 && &head[257..262] == b"ustar" {
        Format::Tar
    } else {
        Format::Raw
    }
}

/// A path inside an install root, from a publisher-supplied relative path like
/// `./bin/agent`. Absolute paths and `..` are refused, so a manifest can never
/// point the launcher outside the directory it was unpacked into.
pub fn relative_inside(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    let mut out = root.to_path_buf();
    let mut any = false;
    for component in Path::new(rel).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                out.push(part);
                any = true;
            }
            _ => bail!("'{rel}' must be a relative path inside the release"),
        }
    }
    if !any {
        bail!("'{rel}' does not name a file");
    }
    Ok(out)
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Stream `url` into `dest`, returning the SHA-256 of what was written. When
/// `expected` is given the file is removed and an error returned on mismatch.
pub async fn download(url: &str, dest: &Path, expected: Option<&str>) -> Result<String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(120))
        .user_agent(concat!("enoxian/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?;
    if !response.status().is_success() {
        bail!("download of {url} returned {}", response.status());
    }
    if response.content_length().unwrap_or(0) > MAX_DOWNLOAD_BYTES {
        bail!("release archive is larger than the 2 GiB limit");
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = response.chunk().await.context("reading the download")? {
        written += chunk.len() as u64;
        if written > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = std::fs::remove_file(dest);
            bail!("release archive is larger than the 2 GiB limit");
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    let actual = hex::encode(hasher.finalize());
    if let Some(expected) = expected {
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(dest);
            bail!("checksum mismatch for {url}: expected {expected}, got {actual}");
        }
    }
    Ok(actual)
}

/// Unpack `file` into `into`. A raw download is moved to `raw_target` (the
/// release's executable path), since there is nothing to unpack.
pub fn extract(file: &Path, into: &Path, raw_target: &Path) -> Result<()> {
    let mut head = [0u8; 512];
    let n = {
        let mut f = std::fs::File::open(file)?;
        read_up_to(&mut f, &mut head)?
    };
    let open = || std::fs::File::open(file).map(std::io::BufReader::new);
    match sniff(&head[..n]) {
        Format::TarGz => unpack_tar(flate2::read::GzDecoder::new(open()?), into),
        Format::TarBz2 => unpack_tar(bzip2::read::BzDecoder::new(open()?), into),
        Format::Tar => unpack_tar(open()?, into),
        Format::Zip => unpack_zip(open()?, into),
        Format::TarXz => bail!("xz-compressed releases are not supported yet"),
        Format::Raw => {
            if let Some(parent) = raw_target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(file, raw_target)
                .or_else(|_| std::fs::copy(file, raw_target).map(|_| ()))
                .with_context(|| format!("placing {}", raw_target.display()))
        }
    }
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

fn unpack_tar(reader: impl Read, into: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    // Keeps each entry's rwx mode but strips setuid/setgid/sticky bits, which a
    // third-party release has no business setting.
    archive.set_preserve_permissions(false);
    archive.set_overwrite(true);
    for entry in archive.entries().context("reading the release archive")? {
        let mut entry = entry.context("reading the release archive")?;
        // `unpack_in` refuses entries that would land outside `into`,
        // including through a symlink planted by an earlier entry.
        let inside = entry
            .unpack_in(into)
            .with_context(|| format!("unpacking into {}", into.display()))?;
        if !inside {
            bail!("release archive contains a path outside its directory");
        }
    }
    Ok(())
}

fn unpack_zip(reader: impl Read + Seek, into: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(reader).context("reading the release zip")?;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        if file.enclosed_name().is_none() {
            let name = file.name().map(|n| n.into_owned()).unwrap_or_default();
            bail!("release zip contains an unsafe path: {name}");
        }
    }
    archive
        .extract(into)
        .with_context(|| format!("unpacking into {}", into.display()))
}

/// Make sure the launcher can run the unpacked executable. Archives built on
/// Windows often lose the mode bits.
pub fn ensure_executable(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("the release does not contain {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sniffs_formats_by_content() {
        assert_eq!(sniff(&[0x1f, 0x8b, 8, 0]), Format::TarGz);
        assert_eq!(sniff(b"BZh91AY"), Format::TarBz2);
        assert_eq!(sniff(b"PK\x03\x04rest"), Format::Zip);
        assert_eq!(sniff(&[0xfd, b'7', b'z', b'X', b'Z', 0, 1]), Format::TarXz);
        assert_eq!(sniff(b"\x7fELF\x02\x01"), Format::Raw);
        assert_eq!(sniff(b"\xcf\xfa\xed\xfe"), Format::Raw);
    }

    #[test]
    fn relative_paths_stay_inside_the_root() {
        let root = Path::new("/r");
        assert_eq!(
            relative_inside(root, "./bin/agent").unwrap(),
            Path::new("/r/bin/agent")
        );
        assert_eq!(
            relative_inside(root, "agent").unwrap(),
            Path::new("/r/agent")
        );
        assert!(relative_inside(root, "../agent").is_err());
        assert!(relative_inside(root, "/usr/bin/agent").is_err());
        assert!(relative_inside(root, "./").is_err());
        assert!(relative_inside(root, "a/../../b").is_err());
    }

    #[test]
    fn checks_sha256_shape() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }

    fn tar_gz_with(path: &str, body: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, body).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn unpacks_a_tar_gz_and_marks_the_command_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("dl");
        std::fs::write(&file, tar_gz_with("pkg/bin/agent", b"#!/bin/sh\n")).unwrap();
        let into = tmp.path().join("root");
        std::fs::create_dir_all(&into).unwrap();
        let exe = relative_inside(&into, "./pkg/bin/agent").unwrap();
        extract(&file, &into, &exe).unwrap();
        ensure_executable(&exe).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111,
                0
            );
        }
    }

    #[test]
    fn unpacks_a_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("dl");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            zip.start_file("agent.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"MZ").unwrap();
            zip.finish().unwrap();
        }
        let into = tmp.path().join("root");
        std::fs::create_dir_all(&into).unwrap();
        let exe = into.join("agent.exe");
        extract(&file, &into, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ");
    }

    #[test]
    fn places_a_raw_executable_at_its_command_path() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("dl");
        std::fs::write(&file, b"\x7fELF\x02\x01\x01").unwrap();
        let into = tmp.path().join("root");
        std::fs::create_dir_all(&into).unwrap();
        let exe = relative_inside(&into, "./sigit-linux-amd64").unwrap();
        extract(&file, &into, &exe).unwrap();
        assert!(exe.is_file());
        assert!(!file.exists());
    }
}
