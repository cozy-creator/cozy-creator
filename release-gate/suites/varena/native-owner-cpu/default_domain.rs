// Appended to the exact domain source in the output-only host proof. Real files;
// this never calls device_descriptor, initializes NVML or claims GPU discovery.
pub(super) fn default_domain_proof(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = default_directory().unwrap();
    let before = fs::metadata(&path).unwrap();
    assert_eq!(default_directory().unwrap(), path);
    assert_eq!(fs::metadata(&path).unwrap().ino(), before.ino());
    assert_eq!(before.mode() & 0o7777, 0o700);
    let private = root.join("directory-race");
    let mut callers = Vec::new();
    for _ in 0..16 {
        let private = private.clone();
        let uid = before.uid();
        callers.push(std::thread::spawn(move || {
            prepare_directory(&private, uid).unwrap()
        }));
    }
    for caller in callers {
        caller.join().unwrap();
    }
    assert_eq!(fs::metadata(&private).unwrap().mode() & 0o7777, 0o700);
    let visible = root.join("wrong-mode");
    fs::create_dir(&visible).unwrap();
    fs::set_permissions(&visible, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(prepare_directory(&visible, before.uid()).is_err());
    assert_eq!(fs::metadata(&visible).unwrap().mode() & 0o7777, 0o755);
    let link = root.join("symlink");
    std::os::unix::fs::symlink(&private, &link).unwrap();
    assert!(prepare_directory(&link, before.uid()).is_err());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let file = root.join("regular-file");
    fs::write(&file, b"preserve this real file").unwrap();
    assert!(prepare_directory(&file, before.uid()).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"preserve this real file");
}
