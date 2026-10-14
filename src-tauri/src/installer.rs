//! Download → verify → extract → swap.
//!
//! A version is extracted into `versions/<ver>.partial` and renamed into place
//! only after its checksum, its signature and its entry executable all check
//! out, so a failed or interrupted install never touches the version that runs.
//! Signatures use the launcher's own ed25519 (minisign) key, the same one
//! Tauri's updater uses: `.sig` files from `tauri signer sign`, public key in
//! `tauri.conf.json` → `plugins.updater.pubkey`.

use crate::manifest::Release;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::StreamExt;
use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(desktop)]
use {
    crate::manifest::parse_version,
    crate::store::{AppPaths, InstallState},
    std::collections::BTreeSet,
    std::path::{Component, PathBuf},
};

pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let resp = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download failed: {e}"))?;
    let total = resp.content_length();
    let tmp = dest.with_extension("part");
    let mut file =
        File::create(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    let mut stream = resp.bytes_stream();
    let mut done = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("download interrupted: {e}"))?;
        file.write_all(&chunk)
            .map_err(|e| format!("cannot write download: {e}"))?;
        done += chunk.len() as u64;
        on_progress(done, total);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// Tauri stores keys and signatures as base64 of the minisign text form.
fn decode_b64_text(value: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|e| format!("bad base64: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("bad text: {e}"))
}

pub fn verify_archive(archive: &Path, release: &Release, pubkey_b64: &str) -> Result<(), String> {
    let pk = PublicKey::decode(&decode_b64_text(pubkey_b64)?)
        .map_err(|e| format!("launcher public key is invalid: {e}"))?;
    let sig = Signature::decode(&decode_b64_text(&release.signature)?)
        .map_err(|e| format!("release signature is invalid: {e}"))?;
    let mut verifier = pk
        .verify_stream(&sig)
        .map_err(|_| "release was not signed with the Yegobox release key".to_string())?;
    let mut sha = Sha256::new();
    let mut file = File::open(archive).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        verifier.update(&buf[..n]);
    }
    let digest: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !digest.eq_ignore_ascii_case(release.sha256.trim()) {
        return Err("download is corrupted (checksum mismatch); try again".into());
    }
    verifier
        .finalize()
        .map_err(|_| "release was not signed with the Yegobox release key".to_string())
}

#[cfg(desktop)]
/// The manifest's `entry` must stay inside the version folder.
pub fn safe_entry(entry: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(entry);
    let ok = !entry.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then_some(path)
        .ok_or_else(|| format!("unsafe entry path in catalog: {entry:?}"))
}

#[cfg(desktop)]
pub fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let file = File::open(archive).map_err(|e| e.to_string())?;
    if name.ends_with(".zip") {
        // ZipArchive::extract rejects entries that escape `dest`.
        zip::ZipArchive::new(file)
            .and_then(|mut z| z.extract(dest))
            .map_err(|e| format!("cannot unpack archive: {e}"))
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        // tar keeps the symlinks a macOS .app is full of, and the exec bits.
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.unpack(dest)
            .map_err(|e| format!("cannot unpack archive: {e}"))
    } else {
        Err(format!("unsupported archive type: {name}"))
    }
}

#[cfg(desktop)]
/// Verify and extract `archive`, then make it current, or stage it for the
/// next launch when the app is running. Returns the new state.
pub fn stage(
    paths: &AppPaths,
    release: &Release,
    archive: &Path,
    pubkey_b64: &str,
    app_running: bool,
) -> Result<InstallState, String> {
    let version = parse_version(&release.version)
        .ok_or_else(|| format!("bad version in catalog: {}", release.version))?
        .to_string();
    let entry = safe_entry(&release.entry)?;
    let mut state = paths.load();
    if app_running && state.current.as_deref() == Some(version.as_str()) {
        return Err(format!("{version} is open; close it before reinstalling"));
    }

    verify_archive(archive, release, pubkey_b64)?;

    let versions = paths.versions();
    fs::create_dir_all(&versions).map_err(|e| e.to_string())?;
    let partial = versions.join(format!("{version}.partial"));
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
    if let Err(e) = extract(archive, &partial) {
        let _ = fs::remove_dir_all(&partial);
        return Err(e);
    }
    if !partial.join(&entry).exists() {
        let _ = fs::remove_dir_all(&partial);
        return Err(format!("archive does not contain {}", entry.display()));
    }
    let final_dir = paths.version(&version);
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|e| e.to_string())?;
    }
    fs::rename(&partial, &final_dir).map_err(|e| e.to_string())?;

    state.entries.insert(version.clone(), release.entry.clone());
    if app_running && state.current.is_some() {
        state.staged = Some(version);
    } else {
        promote(&mut state, version);
    }
    paths.save(&state).map_err(|e| e.to_string())?;
    prune(paths, &mut state);
    let _ = fs::remove_file(archive);
    Ok(state)
}

#[cfg(desktop)]
fn promote(state: &mut InstallState, version: String) {
    if state.current.as_deref() != Some(version.as_str()) {
        state.previous = state.current.take();
        state.current = Some(version);
    }
    state.staged = None;
}

#[cfg(desktop)]
/// Swap in a staged version. Call only while the app is not running.
pub fn apply_staged(paths: &AppPaths) -> Result<bool, String> {
    let mut state = paths.load();
    let Some(version) = state.staged.take() else {
        return Ok(false);
    };
    if paths.entry_path(&state, &version).is_none() {
        paths.save(&state).map_err(|e| e.to_string())?;
        return Ok(false);
    }
    promote(&mut state, version);
    paths.save(&state).map_err(|e| e.to_string())?;
    prune(paths, &mut state);
    Ok(true)
}

#[cfg(desktop)]
/// Go back to `previous`. The abandoned version is skipped by auto-update
/// until a newer one is published.
pub fn rollback(paths: &AppPaths) -> Result<String, String> {
    let mut state = paths.load();
    let previous = state
        .previous
        .clone()
        .filter(|v| paths.entry_path(&state, v).is_some())
        .ok_or("no earlier version to roll back to")?;
    state.skipped = state.current.take();
    state.current = Some(previous.clone());
    state.previous = None;
    state.staged = None;
    state.last_good = Some(previous.clone());
    paths.save(&state).map_err(|e| e.to_string())?;
    prune(paths, &mut state);
    Ok(previous)
}

#[cfg(desktop)]
/// Delete every version folder except current, previous and staged.
/// Failures are ignored: Windows can hold a file open, and the next prune retries.
pub fn prune(paths: &AppPaths, state: &mut InstallState) {
    let keep: BTreeSet<&str> = [&state.current, &state.previous, &state.staged]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    if let Ok(dir) = fs::read_dir(paths.versions()) {
        for item in dir.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if !keep.contains(name.as_str()) {
                let _ = fs::remove_dir_all(item.path());
            }
        }
    }
    let before = state.entries.len();
    let keep: BTreeSet<String> = keep.into_iter().map(String::from).collect();
    state.entries.retain(|v, _| keep.contains(v));
    if state.entries.len() != before {
        let _ = paths.save(state);
    }
}

#[cfg(all(test, desktop))]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Keys {
        kp: minisign::KeyPair,
        pubkey_b64: String,
    }

    fn keys() -> Keys {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pubkey_b64 = STANDARD.encode(kp.pk.to_box().unwrap().into_string());
        Keys { kp, pubkey_b64 }
    }

    fn sign(keys: &Keys, data: &[u8]) -> String {
        let sig = minisign::sign(
            Some(&keys.kp.pk),
            &keys.kp.sk,
            Cursor::new(data),
            None,
            None,
        )
        .unwrap();
        STANDARD.encode(sig.into_string())
    }

    fn sha_hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A zip holding `entry` plus a data file, signed with `keys`.
    fn zip_release(dir: &Path, keys: &Keys, version: &str, entry: &str) -> (Release, PathBuf) {
        let path = dir.join(format!("app-{version}.zip"));
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file(entry, opts).unwrap();
        zip.write_all(format!("binary {version}").as_bytes())
            .unwrap();
        zip.start_file("data/flutter_assets/a.txt", opts).unwrap();
        zip.write_all(b"asset").unwrap();
        zip.finish().unwrap();
        let bytes = fs::read(&path).unwrap();
        let release = Release {
            version: version.into(),
            pub_date: String::new(),
            notes: String::new(),
            min_launcher: "0.0.0".into(),
            url: String::new(),
            sha256: sha_hex(&bytes),
            signature: sign(keys, &bytes),
            size: bytes.len() as u64,
            entry: entry.into(),
            package: None,
            version_code: None,
        };
        (release, path)
    }

    fn setup() -> (tempfile::TempDir, AppPaths, Keys) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(&dir.path().join("apps"), "flipper");
        (dir, paths, keys())
    }

    #[test]
    fn install_update_and_prune() {
        let (dir, paths, keys) = setup();
        for v in ["1.0.0", "1.1.0", "1.2.0"] {
            let (r, a) = zip_release(dir.path(), &keys, v, "flipper.exe");
            stage(&paths, &r, &a, &keys.pubkey_b64, false).unwrap();
            assert!(!a.exists(), "archive is removed after install");
        }
        let s = paths.load();
        assert_eq!(s.current.as_deref(), Some("1.2.0"));
        assert_eq!(s.previous.as_deref(), Some("1.1.0"));
        assert!(!paths.version("1.0.0").exists(), "oldest version pruned");
        assert!(!s.entries.contains_key("1.0.0"));
        assert!(paths
            .entry_path(&s, "1.2.0")
            .unwrap()
            .ends_with("1.2.0/flipper.exe"));
        assert!(paths
            .version("1.2.0")
            .join("data/flutter_assets/a.txt")
            .exists());
    }

    #[test]
    fn checksum_mismatch_changes_nothing() {
        let (dir, paths, keys) = setup();
        let (mut r, a) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        r.sha256 = "00".repeat(32);
        let err = stage(&paths, &r, &a, &keys.pubkey_b64, false).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert_eq!(paths.load(), InstallState::default());
        assert!(!paths.version("1.0.0").exists());
    }

    #[test]
    fn foreign_signature_is_rejected() {
        let (dir, paths, keys) = setup();
        let other = super::tests::keys();
        let (mut r, a) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        r.signature = sign(&other, &fs::read(&a).unwrap());
        let err = stage(&paths, &r, &a, &keys.pubkey_b64, false).unwrap_err();
        assert!(err.contains("signed"), "{err}");
        assert_eq!(paths.load().current, None);
    }

    #[test]
    fn tampered_archive_with_matching_checksum_is_rejected() {
        let (dir, paths, keys) = setup();
        let (mut r, a) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        // An attacker who controls the manifest can fix the checksum but not the signature.
        let mut bytes = fs::read(&a).unwrap();
        bytes.extend_from_slice(b"evil");
        fs::write(&a, &bytes).unwrap();
        r.sha256 = sha_hex(&bytes);
        assert!(stage(&paths, &r, &a, &keys.pubkey_b64, false).is_err());
    }

    #[test]
    fn missing_entry_is_rejected_and_cleaned_up() {
        let (dir, paths, keys) = setup();
        let (mut r, a) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        r.entry = "other.exe".into();
        assert!(stage(&paths, &r, &a, &keys.pubkey_b64, false)
            .unwrap_err()
            .contains("other.exe"));
        assert!(fs::read_dir(paths.versions()).unwrap().next().is_none());
    }

    #[test]
    fn running_app_gets_staged_then_applied() {
        let (dir, paths, keys) = setup();
        let (r1, a1) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        stage(&paths, &r1, &a1, &keys.pubkey_b64, false).unwrap();
        let (r2, a2) = zip_release(dir.path(), &keys, "1.1.0", "flipper.exe");
        let s = stage(&paths, &r2, &a2, &keys.pubkey_b64, true).unwrap();
        assert_eq!(s.current.as_deref(), Some("1.0.0"));
        assert_eq!(s.staged.as_deref(), Some("1.1.0"));

        assert!(apply_staged(&paths).unwrap());
        let s = paths.load();
        assert_eq!(s.current.as_deref(), Some("1.1.0"));
        assert_eq!(s.previous.as_deref(), Some("1.0.0"));
        assert_eq!(s.staged, None);
        assert!(!apply_staged(&paths).unwrap());
    }

    #[test]
    fn reinstalling_the_open_version_is_refused() {
        let (dir, paths, keys) = setup();
        let (r, a) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        stage(&paths, &r, &a, &keys.pubkey_b64, false).unwrap();
        let (r, a) = zip_release(dir.path(), &keys, "1.0.0", "flipper.exe");
        assert!(stage(&paths, &r, &a, &keys.pubkey_b64, true)
            .unwrap_err()
            .contains("close"));
    }

    #[test]
    fn rollback_restores_previous_and_skips_bad_version() {
        let (dir, paths, keys) = setup();
        for v in ["1.0.0", "1.1.0"] {
            let (r, a) = zip_release(dir.path(), &keys, v, "flipper.exe");
            stage(&paths, &r, &a, &keys.pubkey_b64, false).unwrap();
        }
        assert_eq!(rollback(&paths).unwrap(), "1.0.0");
        let s = paths.load();
        assert_eq!(s.current.as_deref(), Some("1.0.0"));
        assert_eq!(s.skipped.as_deref(), Some("1.1.0"));
        assert!(!paths.version("1.1.0").exists());
        assert!(rollback(&paths).is_err(), "nothing left to roll back to");
    }

    #[tokio::test]
    async fn download_streams_to_disk_and_reports_progress() {
        use std::io::{BufRead, BufReader};
        let body = vec![7u8; 3 << 20];
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = body.clone();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                served.len()
            )
            .unwrap();
            sock.write_all(&served).unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("downloads/app.zip");
        let mut calls = Vec::new();
        download(
            &reqwest::Client::new(),
            &format!("http://127.0.0.1:{port}/app.zip"),
            &dest,
            |d, t| calls.push((d, t)),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert!(!dest.with_extension("part").exists());
        assert_eq!(
            calls.last().copied(),
            Some((body.len() as u64, Some(body.len() as u64)))
        );
    }

    #[tokio::test]
    async fn http_errors_fail_the_download() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let _ = sock.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("x.zip");
        let err = download(
            &reqwest::Client::new(),
            &format!("http://127.0.0.1:{port}/x.zip"),
            &dest,
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("404"), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn entry_paths_cannot_escape() {
        assert!(safe_entry("flipper.exe").is_ok());
        assert!(safe_entry("flipper.app").is_ok());
        assert!(safe_entry("bin/flipper").is_ok());
        assert!(safe_entry("../flipper").is_err());
        assert!(safe_entry("/usr/bin/flipper").is_err());
        assert!(safe_entry("./flipper").is_err());
        assert!(safe_entry("").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn tar_gz_keeps_symlinks_and_exec_bits() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, paths, keys) = setup();
        let archive = dir.path().join("flipper-darwin.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            let body = b"#!/bin/sh\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(
                &mut h,
                "flipper.app/Contents/Frameworks/A.framework/Versions/A/A",
                &body[..],
            )
            .unwrap();
            let mut l = tar::Header::new_gnu();
            l.set_entry_type(tar::EntryType::Symlink);
            l.set_size(0);
            l.set_cksum();
            tar.append_link(
                &mut l,
                "flipper.app/Contents/Frameworks/A.framework/A",
                "Versions/A/A",
            )
            .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let bytes = fs::read(&archive).unwrap();
        let release = Release {
            version: "1.0.0".into(),
            pub_date: String::new(),
            notes: String::new(),
            min_launcher: "0.0.0".into(),
            url: String::new(),
            sha256: sha_hex(&bytes),
            signature: sign(&keys, &bytes),
            size: bytes.len() as u64,
            entry: "flipper.app".into(),
            package: None,
            version_code: None,
        };
        stage(&paths, &release, &archive, &keys.pubkey_b64, false).unwrap();
        let fw = paths
            .version("1.0.0")
            .join("flipper.app/Contents/Frameworks/A.framework");
        assert!(fs::symlink_metadata(fw.join("A"))
            .unwrap()
            .file_type()
            .is_symlink());
        let mode = fs::metadata(fw.join("Versions/A/A"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "exec bit kept");
    }
}
