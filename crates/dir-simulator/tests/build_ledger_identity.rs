#[path = "../build_identity.rs"]
mod build_identity;

#[test]
fn ledger_identity_requires_a_real_document_version() {
    assert_eq!(
        build_identity::ledger_identity(None),
        ("not-present".into(), "not-present".into())
    );
    let bytes = "# ledger\n文書バージョン：`1.1.0`\n".as_bytes();
    let (hash, version) = build_identity::ledger_identity(Some(bytes));
    assert_eq!(version, "1.1.0");
    assert_eq!(
        hash,
        "9db57e22cc5efc5af962d8841b983d6c0e7f9de3627dc13f99e9d7dc8d2fe495"
    );
    let changed = "# changed ledger\n文書バージョン：`1.1.0`\n".as_bytes();
    assert_ne!(hash, build_identity::ledger_identity(Some(changed)).0);
    for invalid in [
        "no version",
        "文書バージョン：`unknown`",
        "文書バージョン：`1.1`",
        "文書バージョン：`1.x.0`",
    ] {
        assert_eq!(
            build_identity::ledger_identity(Some(invalid.as_bytes())).1,
            "unknown"
        );
    }
    assert_eq!(
        build_identity::ledger_identity(Some(&[0xff])).1,
        "unknown"
    );
}
