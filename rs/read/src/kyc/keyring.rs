use std::{collections::BTreeMap, fmt, io::Read, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use zeroize::Zeroizing;

use super::vault::VaultError;

const MAX_KEY_FILE_BYTES: u64 = 1024 * 1024;

pub struct KeyRing {
    pub(crate) active: String,
    keys: BTreeMap<String, Zeroizing<Vec<u8>>>,
}

impl fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyRing([REDACTED])")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncodedRing {
    active: String,
    #[serde(deserialize_with = "unique_keys")]
    keys: BTreeMap<String, Zeroizing<String>>,
}

fn unique_keys<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Zeroizing<String>>, D::Error> {
    struct Keys;
    impl<'de> Visitor<'de> for Keys {
        type Value = BTreeMap<String, Zeroizing<String>>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("unique keys")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut keys = BTreeMap::new();
            while let Some((id, value)) = map.next_entry::<String, String>()? {
                let value = Zeroizing::new(value);
                if keys.insert(id, value).is_some() {
                    return Err(serde::de::Error::custom("invalid key ring"));
                }
            }
            Ok(keys)
        }
    }
    deserializer.deserialize_map(Keys)
}

impl KeyRing {
    pub fn parse(raw: &[u8]) -> Result<Self, VaultError> {
        if raw.len() > MAX_KEY_FILE_BYTES as usize {
            return Err(VaultError);
        }
        let encoded: EncodedRing = serde_json::from_slice(raw).map_err(|_| VaultError)?;
        let mut keys = BTreeMap::new();
        for (id, text) in encoded.keys {
            if id.is_empty()
                || id.len() > 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(VaultError);
            }
            let key = Zeroizing::new(STANDARD.decode(text.as_bytes()).map_err(|_| VaultError)?);
            if key.len() != 32 || STANDARD.encode(key.as_slice()).as_bytes() != text.as_bytes() {
                return Err(VaultError);
            }
            keys.insert(id, key);
        }
        if !keys.contains_key(&encoded.active) {
            return Err(VaultError);
        }
        Ok(Self {
            active: encoded.active,
            keys,
        })
    }

    pub fn load(path: &Path) -> Result<Self, VaultError> {
        let file = std::fs::File::open(path).map_err(|_| VaultError)?;
        let mut raw = Zeroizing::new(Vec::new());
        file.take(MAX_KEY_FILE_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|_| VaultError)?;
        Self::parse(&raw)
    }

    /// Call once during startup. Failure disables storage, not service readiness.
    pub fn load_runtime(
        credential_directory: Option<&Path>,
        explicit_path: Option<&Path>,
        environment: &str,
    ) -> Result<Self, VaultError> {
        if let Some(directory) = credential_directory {
            return Self::load(&directory.join("kyc-document-keys"));
        }
        if matches!(environment, "development" | "test") {
            return Self::load(explicit_path.ok_or(VaultError)?);
        }
        Err(VaultError)
    }

    pub(crate) fn key(&self, id: &str) -> Result<&[u8], VaultError> {
        self.keys
            .get(id)
            .map(|key| key.as_slice())
            .ok_or(VaultError)
    }
}
