use libpm::PackageManager;

#[test]
fn install_rejects_an_unknown_app_id() {
    let manager = PackageManager::init().expect("package manager should initialize");
    let error = manager
        .install(u64::MAX)
        .expect_err("an unknown app ID must not reach a package manager");

    assert_eq!(
        error.to_string(),
        format!("app {} is not in the package catalog", u64::MAX)
    );
}

#[test]
fn pm_root_rejects_commands_other_than_pacman_and_nix() {
    let status = std::process::Command::new("sh")
        .args([concat!(env!("CARGO_MANIFEST_DIR"), "/../pm-root"), "id"])
        .env("PKEXEC_UID", "0")
        .status()
        .expect("sh should start");

    assert_eq!(status.code(), Some(2));
}
