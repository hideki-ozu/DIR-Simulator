use sha2::{Digest, Sha256};

pub fn ledger_identity(bytes: Option<&[u8]>) -> (String, String) {
    let Some(bytes) = bytes else {
        return ("not-present".into(), "not-present".into());
    };
    let hash = format!("{:x}", Sha256::digest(bytes));
    let version = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("文書バージョン：`"))
        })
        .and_then(|value| value.strip_suffix('`'))
        .filter(|version| {
            let parts = version.split('.').collect::<Vec<_>>();
            parts.len() == 3
                && parts.iter().all(|part| {
                    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
        .unwrap_or("unknown")
        .to_owned();
    (hash, version)
}
