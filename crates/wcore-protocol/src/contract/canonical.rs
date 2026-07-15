use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::ContractResult;

/// Serialize one JSON value with recursively sorted object keys and one LF.
pub fn canonical_json(value: &Value) -> ContractResult<Vec<u8>> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(sorted).collect()),
            Value::Object(values) => {
                let ordered = values
                    .iter()
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect::<BTreeMap<_, _>>();
                Value::Object(ordered.into_iter().collect())
            }
            scalar => scalar.clone(),
        }
    }

    let mut bytes = serde_json::to_vec(&sorted(value))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Hash sorted `relative path + NUL + exact bytes` entries.
pub fn digest_named_bytes<'a>(entries: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut ordered = entries.into_iter().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|(path, _)| *path);

    let mut hasher = Sha256::new();
    for (path, bytes) in ordered {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
    }
    format!("sha256:{:x}", hasher.finalize())
}
