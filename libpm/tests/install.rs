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
