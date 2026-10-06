use state_fs::{FileSystem, LockMode, Metadata, OpenMode, PhysicalFs};
use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

#[test]
fn files_reopen_sync_rename_truncate_and_keep_old_handles() -> io::Result<()> {
    let _serial = super::common::serial();
    let directory = tempfile::tempdir()?;
    let fs = PhysicalFs;
    let root = fs.prepare_root(&directory.path().join("nested/root"))?;
    let source = root.join("source");
    let target = root.join("target");
    let mut file = fs.open(&source, OpenMode::CreateNew)?;
    file.write_all(b"abcdef")?;
    file.set_len(2)?;
    file.set_len(5)?;
    file.sync_all()?;
    assert!(
        matches!(fs.open(&source, OpenMode::CreateNew), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
    );
    fs.rename(&source, &target)?;
    fs.sync_dir(&root)?;
    let mut reader = fs.open(&target, OpenMode::Read)?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"ab\0\0\0");
    assert!(reader.write_all(b"x").is_err());
    assert!(reader.set_len(0).is_err());
    drop(reader);
    fs.remove_file(&target)?;
    fs.sync_dir(&root)?;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"ab\0\0\0");
    assert!(matches!(fs.metadata(&target), Err(e) if e.kind() == io::ErrorKind::NotFound));
    Ok(())
}

#[cfg(unix)]
#[test]
fn metadata_does_not_follow_symlinks() -> io::Result<()> {
    let _serial = super::common::serial();
    let directory = tempfile::tempdir()?;
    let fs = PhysicalFs;
    let root = fs.prepare_root(directory.path())?;
    let target = root.join("target");
    fs.open(&target, OpenMode::CreateNew)?
        .write_all(b"unchanged")?;
    let link = root.join("link");
    std::os::unix::fs::symlink(&target, &link)?;
    assert!(matches!(fs.metadata(&link)?, Metadata::Other));
    fs.remove_file(&target)?;
    assert!(matches!(fs.metadata(&link)?, Metadata::Other));
    Ok(())
}

#[test]
#[ignore = "subprocess entry point"]
fn lock_probe() -> Result<(), Box<dyn std::error::Error>> {
    let _serial = super::common::serial();
    let Some(path) = std::env::var_os("STATE_FS_LOCK_PROBE") else {
        return Ok(());
    };
    let mode = match std::env::var("STATE_FS_LOCK_MODE")?.as_str() {
        "shared" => LockMode::Shared,
        "exclusive" => LockMode::Exclusive,
        _ => return Err("invalid lock probe mode".into()),
    };
    let available = match std::env::var("STATE_FS_LOCK_AVAILABLE")?.as_str() {
        "yes" => true,
        "no" => false,
        _ => return Err("invalid lock availability".into()),
    };
    match PhysicalFs.lock(Path::new(&path), mode) {
        Ok(_) => assert!(available),
        Err(error) => {
            assert!(!available);
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        }
    }
    Ok(())
}

#[test]
fn locks_coordinate_processes_and_release_on_drop() -> Result<(), Box<dyn std::error::Error>> {
    let _serial = super::common::serial();
    let directory = tempfile::tempdir()?;
    let root = PhysicalFs.prepare_root(directory.path())?;
    let lock = root.join("lock");
    let probe = |mode: &str, available: &str| -> Result<(), Box<dyn std::error::Error>> {
        let child = std::process::Command::new(std::env::current_exe()?)
            .args(["--ignored", "--exact", "filesystem::lock_probe"])
            .env("STATE_FS_LOCK_PROBE", &lock)
            .env("STATE_FS_LOCK_MODE", mode)
            .env("STATE_FS_LOCK_AVAILABLE", available)
            .output()?;
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(
            String::from_utf8_lossy(&child.stdout).contains("1 passed"),
            "lock probe did not run"
        );
        Ok(())
    };
    let reader = PhysicalFs.lock(&lock, LockMode::Shared)?;
    probe("shared", "yes")?;
    probe("exclusive", "no")?;
    drop(reader);
    let writer = PhysicalFs.lock(&lock, LockMode::Exclusive)?;
    probe("shared", "no")?;
    probe("exclusive", "no")?;
    drop(writer);
    probe("exclusive", "yes")?;
    Ok(())
}
