use std::fmt;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::Serialize;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::keyring::KeyRing;

#[derive(Debug, Clone, Copy)]
pub struct VaultError;
impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Evidence storage operation failed")
    }
}
impl std::error::Error for VaultError {}

#[derive(Clone, Copy, Serialize)]
pub enum DocumentKind {
    #[serde(rename = "IDENTITY")]
    Identity,
    #[serde(rename = "COMPANY_REGISTRATION")]
    CompanyRegistration,
}
impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "IDENTITY",
            Self::CompanyRegistration => "COMPANY_REGISTRATION",
        }
    }
    pub fn from_storage(value: &str) -> Result<Self, VaultError> {
        match value {
            "IDENTITY" => Ok(Self::Identity),
            "COMPANY_REGISTRATION" => Ok(Self::CompanyRegistration),
            _ => Err(VaultError),
        }
    }
}

pub struct DocumentMetadata {
    pub id: Uuid,
    pub submission_id: Uuid,
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub kind: DocumentKind,
    pub content_type: String,
}

pub struct EncryptedDocument {
    pub metadata: DocumentMetadata,
    pub size_bytes: i32,
    pub key_id: String,
    pub wrapped_key: Vec<u8>,
    pub key_nonce: Vec<u8>,
    pub data_nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}
impl fmt::Debug for EncryptedDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncryptedDocument([REDACTED])")
    }
}

fn data_aad(metadata: &DocumentMetadata) -> Result<Vec<u8>, VaultError> {
    if !matches!(
        metadata.content_type.as_str(),
        "application/pdf" | "image/jpeg" | "image/png"
    ) {
        return Err(VaultError);
    }
    serde_json::to_vec(&[
        "kyc-document-v1".to_owned(),
        metadata.id.to_string(),
        metadata.submission_id.to_string(),
        metadata.user_id.to_string(),
        metadata.organization_id.to_string(),
        metadata.kind.as_str().to_owned(),
        metadata.content_type.clone(),
    ])
    .map_err(|_| VaultError)
}
fn wrap_aad(id: Uuid, key: &str) -> Result<Vec<u8>, VaultError> {
    serde_json::to_vec(&["kyc-key-v1".to_owned(), id.to_string(), key.to_owned()])
        .map_err(|_| VaultError)
}

pub fn encrypt(
    keys: &KeyRing,
    metadata: DocumentMetadata,
    plaintext: &[u8],
) -> Result<EncryptedDocument, VaultError> {
    if plaintext.is_empty() {
        return Err(VaultError);
    }
    let size_bytes = i32::try_from(plaintext.len()).map_err(|_| VaultError)?;
    let mut data_key = Zeroizing::new([0u8; 32]);
    getrandom::fill(data_key.as_mut()).map_err(|_| VaultError)?;
    let mut data_nonce = [0u8; 12];
    let mut key_nonce = [0u8; 12];
    getrandom::fill(&mut data_nonce).map_err(|_| VaultError)?;
    getrandom::fill(&mut key_nonce).map_err(|_| VaultError)?;
    let data_cipher = Aes256Gcm::new_from_slice(data_key.as_ref()).map_err(|_| VaultError)?;
    let ciphertext = data_cipher
        .encrypt(
            &Nonce::from(data_nonce),
            Payload {
                msg: plaintext,
                aad: &data_aad(&metadata)?,
            },
        )
        .map_err(|_| VaultError)?;
    let wrapping_cipher =
        Aes256Gcm::new_from_slice(keys.key(&keys.active)?).map_err(|_| VaultError)?;
    let wrapped_key = wrapping_cipher
        .encrypt(
            &Nonce::from(key_nonce),
            Payload {
                msg: data_key.as_ref(),
                aad: &wrap_aad(metadata.id, &keys.active)?,
            },
        )
        .map_err(|_| VaultError)?;
    Ok(EncryptedDocument {
        metadata,
        size_bytes,
        key_id: keys.active.clone(),
        wrapped_key,
        key_nonce: key_nonce.to_vec(),
        data_nonce: data_nonce.to_vec(),
        ciphertext,
    })
}

pub fn decrypt(keys: &KeyRing, row: &EncryptedDocument) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let key_nonce: [u8; 12] = row
        .key_nonce
        .as_slice()
        .try_into()
        .map_err(|_| VaultError)?;
    let data_nonce: [u8; 12] = row
        .data_nonce
        .as_slice()
        .try_into()
        .map_err(|_| VaultError)?;
    let wrapping_cipher =
        Aes256Gcm::new_from_slice(keys.key(&row.key_id)?).map_err(|_| VaultError)?;
    let data_key = Zeroizing::new(
        wrapping_cipher
            .decrypt(
                &Nonce::from(key_nonce),
                Payload {
                    msg: &row.wrapped_key,
                    aad: &wrap_aad(row.metadata.id, &row.key_id)?,
                },
            )
            .map_err(|_| VaultError)?,
    );
    let cipher = Aes256Gcm::new_from_slice(&data_key).map_err(|_| VaultError)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &Nonce::from(data_nonce),
                Payload {
                    msg: &row.ciphertext,
                    aad: &data_aad(&row.metadata)?,
                },
            )
            .map_err(|_| VaultError)?,
    );
    if plaintext.is_empty() || usize::try_from(row.size_bytes).ok() != Some(plaintext.len()) {
        return Err(VaultError);
    }
    Ok(plaintext)
}
