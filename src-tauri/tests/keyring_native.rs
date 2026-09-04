#[cfg(target_os = "macos")]
#[test]
fn macos_keychain_persists_across_entries() {
    let account = format!(
        "native-keyring-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let service = "xyz.ownpath.sift.tests";
    let first = keyring::Entry::new(service, &account).unwrap();
    first.set_password("non-secret-test-value").unwrap();

    // A second Entry must see the value. The featureless keyring backend used
    // before this fix creates isolated MockCredential values and fails here.
    let second = keyring::Entry::new(service, &account).unwrap();
    assert_eq!(second.get_password().unwrap(), "non-secret-test-value");
    second.delete_credential().unwrap();
}
